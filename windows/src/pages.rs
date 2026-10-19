//! ULTRA's own pages, opened by typing ultra://<name> into the address bar (like Chrome's
//! chrome://settings). Some open one of ULTRA's panels; the rest are documents shown in the tab
//! itself (the webview gets the page as HTML; the tab keeps the ultra:// address).

/// Every page, for ultra://about and the address bar's suggestions: (name, what it shows).
pub const PAGES: &[(&str, &str)] = &[
    ("settings", "settings"),
    ("downloads", "downloads"),
    ("chat", "friends & chat"),
    ("about", "about ULTRA and its pages"),
    ("privacy", "privacy policy"),
    ("terms", "terms of use and chat rules"),
    ("licenses", "license and third-party notices"),
];

const PRIVACY: &str = include_str!("../../PRIVACY.md");
const TERMS: &str = include_str!("../../TERMS.md");
const NOTICES: &str = include_str!("../../THIRD_PARTY_NOTICES.md");
const LICENSE: &str = include_str!("../../LICENSE");

/// What was typed, as an ULTRA page address: "ultra://Settings/", "ultra:settings" ->
/// "ultra://settings"; "ultra://" -> about; "ultra://home" -> the new-tab page. None: not ours.
pub fn normalize(input: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    let rest = lower.strip_prefix("ultra://").or_else(|| lower.strip_prefix("ultra:"))?;
    let name = rest.trim_start_matches('/').split(['/', '?', '#']).next().unwrap_or("");
    Some(match name {
        "" => "ultra://about".into(),
        "home" | "newtab" => crate::HOME.into(),
        "friends" => "ultra://chat".into(),
        "license" | "notices" | "credits" => "ultra://licenses".into(),
        n => format!("ultra://{n}"),
    })
}

/// Is this one of ULTRA's document pages (shown in the tab)?
pub fn is_page(url: &str) -> bool {
    url.starts_with("ultra://") && panel(url).is_none()
}

/// The panel an address opens instead of a page (the command that opens it).
pub fn panel(url: &str) -> Option<&'static str> {
    match url.strip_prefix("ultra://")? {
        "settings" => Some("settings"),
        "downloads" => Some("downloads"),
        "chat" => Some("friends"),
        _ => None,
    }
}

/// The page's HTML.
pub fn html(url: &str) -> String {
    let name = url.strip_prefix("ultra://").unwrap_or("");
    let (title, body) = match name {
        "privacy" => ("Privacy Policy", markdown(PRIVACY)),
        "terms" => ("Terms of Use", markdown(TERMS)),
        "licenses" => (
            "Licenses",
            format!(
                "{}<h2 id=\"gpl\">GNU General Public License, version 3</h2><p>ULTRA's own license.</p><pre>{}</pre>",
                markdown(NOTICES),
                escape(LICENSE)
            ),
        ),
        "about" => ("About ULTRA", about()),
        other => (
            "Not found",
            format!("<h1>ultra://{} isn't a page</h1><p>These are:</p>{}", escape(other), list()),
        ),
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title><style>
:root {{ color-scheme: dark; }}
body {{ margin: 0; background: #141414; color: #d6d6d6; font: 15px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif; }}
main {{ max-width: 760px; margin: 0 auto; padding: 48px 28px 80px; }}
h1 {{ font-size: 26px; color: #f2f2f2; margin: 0 0 6px; }}
h2 {{ font-size: 19px; color: #ececec; margin: 34px 0 8px; }}
h3 {{ font-size: 16px; color: #e6e6e6; margin: 24px 0 6px; }}
a {{ color: #9fe3d0; }}
code {{ background: #222; padding: 1px 5px; border-radius: 4px; font-size: 13px; }}
pre {{ background: #1b1b1b; border: 1px solid #262626; border-radius: 6px; padding: 12px; overflow-x: auto; font-size: 12px; line-height: 1.45; white-space: pre-wrap; }}
pre code {{ background: none; padding: 0; }}
table {{ border-collapse: collapse; width: 100%; margin: 12px 0; font-size: 14px; }}
th, td {{ border: 1px solid #2c2c2c; padding: 6px 9px; text-align: left; vertical-align: top; }}
th {{ background: #1c1c1c; color: #eee; }}
.pages a {{ font-family: ui-monospace, Consolas, monospace; }}
.muted {{ color: #8a8a8a; }}
</style></head><body><main>{body}</main><script>
// Links to other ULTRA pages: ULTRA opens them (a page can't navigate to ultra:// itself).
document.addEventListener('click', (e) => {{
  const a = e.target.closest('a[href^="ultra://"]');
  if (a) {{ e.preventDefault(); window.ipc.postMessage('ultra:' + a.getAttribute('href').slice(8)); }}
}});
</script></body></html>"#
    )
}

fn about() -> String {
    format!(
        "<h1>{name}</h1><p class=\"muted\">{version}</p>\
         <p>A fast, minimal, tiling web browser. Free software under the \
         <a href=\"ultra://licenses\">GNU General Public License v3</a>.</p>\
         <h2>ULTRA's pages</h2><p>Type any of these into the address bar.</p>{list}",
        name = escape(crate::edition::NAME),
        version = escape(&crate::updater::version()),
        list = list()
    )
}

fn list() -> String {
    let rows: String = PAGES
        .iter()
        .map(|(n, d)| format!("<tr><td><a href=\"ultra://{n}\">ultra://{n}</a></td><td>{d}</td></tr>"))
        .collect();
    format!("<table class=\"pages\">{rows}</table>")
}

/// Markdown to HTML, with the repository's links between documents pointing at ULTRA's pages.
fn markdown(md: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(md, Options::ENABLE_TABLES));
    out.replace("href=\"PRIVACY.md\"", "href=\"ultra://privacy\"")
        .replace("href=\"TERMS.md\"", "href=\"ultra://terms\"")
        .replace("href=\"THIRD_PARTY_NOTICES.md\"", "href=\"ultra://licenses\"")
        .replace("href=\"LICENSE\"", "href=\"ultra://licenses#gpl\"")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(normalize("ultra://Settings/").as_deref(), Some("ultra://settings"));
        assert_eq!(normalize("ultra:privacy").as_deref(), Some("ultra://privacy"));
        assert_eq!(normalize("ultra://").as_deref(), Some("ultra://about"));
        assert_eq!(normalize("ultra://home").as_deref(), Some(crate::HOME));
        assert_eq!(normalize("https://ultra.example"), None);
        assert_eq!(panel("ultra://settings"), Some("settings"));
        assert!(is_page("ultra://terms") && !is_page("ultra://settings") && !is_page("https://x.com"));
    }

    #[test]
    fn documents_render() {
        let p = html("ultra://privacy");
        assert!(p.contains("<title>Privacy Policy</title>") && p.contains("<table>"));
        assert!(html("ultra://terms").contains("href=\"ultra://privacy\""));
        assert!(html("ultra://licenses").contains("GNU GENERAL PUBLIC LICENSE"));
        assert!(html("ultra://<x>").contains("ultra://&lt;x&gt;"));
    }
}
