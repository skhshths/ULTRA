"""Merge cargo-about output for several crates into one THIRD_PARTY_NOTICES.md (one entry per
distinct license text, listing every crate that uses it)."""
import re, sys, pathlib

out, *inputs = sys.argv[1:]
OWN = {'ultra', 'ultra-exp', 'ultra-server'}
merged = {}  # (name, text) -> set of "crate version"
for f in inputs:
    s = pathlib.Path(f).read_text(encoding='utf8')
    for sec in s.split('\n---\n'):
        m = re.search(r'### (.+?)\n\nUsed by: (.*?)\n\n```\n(.*)\n```', sec, re.S)
        if not m:
            continue
        name, used, text = m.group(1).strip(), m.group(2), m.group(3).strip()
        crates = {c.strip() for c in used.split(', ') if c.strip() and c.split(' ')[0] not in OWN}
        if crates:
            merged.setdefault((name, text), set()).update(crates)

head = '''# Third-party notices

ULTRA is licensed under the [GNU General Public License, version 3](LICENSE). It includes the
work of others, listed here with their licenses.

## Ad-block filter lists

ULTRA ships these lists inside the app and may download newer copies from their publishers.

| List | Source | License |
| --- | --- | --- |
| EasyList, EasyPrivacy | https://easylist.to/ | GPL-3.0 or later, or CC BY-SA 3.0 or later. Credit: **The EasyList authors (https://easylist.to/)** |
| uBlock filters, uBlock filters – Privacy | https://github.com/uBlockOrigin/uAssets | GPL-3.0 (full text in [LICENSE](LICENSE)) |
| Peter Lowe's ad and tracking server list | https://pgl.yoyo.org/adservers/ | Used with the author's general permission to combine and republish the list |

The lists are used unchanged, apart from being compressed into the app. Their sources are at the
addresses above.

## Browser engines

Web pages are displayed by Microsoft Edge WebView2 (Windows) and Apple WebKit (macOS), which are
part of the operating system and not distributed with ULTRA.

## Rust libraries

The Windows app, the Mac app and the chat relay are built from these open-source libraries. Each
license text below is followed by the libraries it covers.
'''
parts = [head]
for (name, text), crates in sorted(merged.items(), key=lambda kv: (kv[0][0], sorted(kv[1])[0])):
    parts.append(f'\n### {name}\n\nUsed by: {", ".join(sorted(crates))}\n\n```\n{text}\n```\n')
pathlib.Path(out).write_text(''.join(parts), encoding='utf8')
print(len(merged), 'license texts,', sum(len(c) for c in merged.values()), 'crate entries')
