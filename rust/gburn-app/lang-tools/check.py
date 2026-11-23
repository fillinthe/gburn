"""Check a language table against the English list: every key once, nothing extra, {tags}, \\n, key symbols kept."""
import re, sys
keys = [l.rstrip('\n') for l in open(sys.argv[1]) if l.strip()]
for path in sys.argv[2:]:
    rows = [l.rstrip('\n') for l in open(path) if l.strip() and not l.startswith('#')]
    bad, got = [], {}
    for r in rows:
        if r.count('\t') != 1:
            bad.append(('tabs', r)); continue
        en, tr = r.split('\t')
        got[en] = tr
        for pat in [r'\{[^}]*\}', r'\\n', '[⇧⌘⌥⌫▲▼◀▶●★↔%]']:
            if sorted(re.findall(pat, en)) != sorted(re.findall(pat, tr)):
                bad.append((pat, en, tr))
        if not tr.strip():
            bad.append(('empty', en))
    missing = [k for k in keys if k not in got]
    extra = [k for k in got if k not in keys]
    print(path.split('/')[-1], 'rows', len(rows), 'missing', len(missing), 'extra', len(extra), 'problems', len(bad))
    for b in bad[:12]:
        print('   ', b)
    for m in missing[:5]:
        print('    missing:', m)
