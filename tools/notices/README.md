# Third-party notices

Regenerates `THIRD_PARTY_NOTICES.md` (the license text of every Rust library in the Windows app,
the Mac app and the chat relay) after dependencies change. Needs `cargo install cargo-about
--features cli`.

```sh
for d in windows mac server; do (cd $d && cargo about generate -c ../tools/notices/about.toml ../tools/notices/about.hbs -o ../target-notices-$d.md); done
python tools/notices/merge.py THIRD_PARTY_NOTICES.md target-notices-windows.md target-notices-mac.md target-notices-server.md
rm target-notices-*.md
```

A new license that isn't in `about.toml`'s `accepted` list stops the run: check it's compatible
with GPL-3.0 before adding it.
