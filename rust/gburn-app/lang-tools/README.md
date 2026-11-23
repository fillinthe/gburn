# Interface translations

`src/lang/<code>.txt` = one `English<TAB>translation` a line (`\n` = a line break). `src/i18n.rs` loads them; `tr("…")` in app.rs.

New or changed interface text:

    python3 -I lang-tools/extract.py src/app.rs > /tmp/strings.txt        # every string the app translates
    python3 -I lang-tools/check.py /tmp/strings.txt src/lang/*.txt          # what each table is missing

Translate the missing lines (keep `{tags}`, `\n`, ⇧ ⌘ symbols, product names) and append them to every table. Keep tool-rail
labels to about 11 characters. Then `cargo test` (the side-panel test renders every language at two widths).
