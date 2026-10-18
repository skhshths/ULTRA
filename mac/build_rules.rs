// Turns the ad-block filter lists (shared with Windows: ../windows/assets/filters) into WebKit
// content-blocker rules, with Brave's converter. Used by build.rs; kept in its own file so it
// can also be run on its own to check what it produces.
//
// Network rules become "block" rules and cosmetic rules become "css-display-none" rules, which
// WebKit applies natively. The result is split into chunks small enough for every macOS ULTRA
// supports (macOS 11 and 12 accept 50,000 rules per list).

pub const CHUNK: usize = 45_000;

/// (chunks of rules as WebKit JSON, number of rules, number of filters that couldn't convert)
pub fn convert(lists: &[String]) -> (Vec<String>, usize, usize) {
    let mut set = adblock::FilterSet::new(true);
    for text in lists {
        set.add_filter_list(text.clone(), adblock::lists::ParseOptions::default());
    }
    let (rules, skipped) = set.into_content_blocking().unwrap_or_default();
    // WebKit rejects a whole list over one rule it can't parse; the converter already drops the
    // ones it knows WebKit won't take.
    let rules: Vec<serde_json::Value> = rules.into_iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
    let n = rules.len();
    // An exception ("ignore-previous-rules") only cancels rules earlier in its own list, so
    // every chunk ends with all of them; the other rules are split between the chunks.
    let (exceptions, rest): (Vec<_>, Vec<_>) = rules.into_iter().partition(|r| r["action"]["type"] == "ignore-previous-rules");
    let per = CHUNK.saturating_sub(exceptions.len()).max(1);
    let chunks = rest
        .chunks(per)
        .map(|c| serde_json::Value::Array(c.iter().chain(&exceptions).cloned().collect()).to_string())
        .collect();
    (chunks, n, skipped.len())
}

/// The filter list texts, in a stable order.
pub fn read_lists(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect())
        .unwrap_or_default();
    paths.sort();
    paths
        .into_iter()
        .map(|p| (p.file_stem().unwrap_or_default().to_string_lossy().to_string(), std::fs::read_to_string(&p).unwrap_or_default()))
        .collect()
}
