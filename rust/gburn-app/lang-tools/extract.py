"""Every interface string app.rs passes through tr() (and the menu / rail / tool strings translated inside helpers),
one per line, \n escaped. Usage: python3 -I extract.py app.rs > strings.txt"""
import os, re, sys
base = os.path.dirname(os.path.abspath(sys.argv[1]))
s = open(sys.argv[1]).read()
head = s[:s.index('#[cfg(test)]\nmod tests')]
S = r'"((?:[^"\\]|\\.)*)"'
keys = []
keys += [m.group(1) for m in re.finditer(r'(?<![A-Za-z0-9_])tr\(' + S + r'\)', head)]
keys += [m.group(1) for m in re.finditer(r'item\(ui, &mut cmd, ' + S, head)]
for m in re.finditer(r'item\(ui, &mut cmd, "[^"]*", "[^"]*", ' + S + ', ' + S, head):
    keys += [m.group(1), m.group(2)]
for m in re.finditer(r'tr\(if [^{]*\{ ' + S + r' \} else \{ ' + S + r' \}\)', head):
    keys += [m.group(1), m.group(2)]
for m in re.finditer(r'item\(ui, &mut cmd, if [^{]*\{ ' + S + r' \} else \{ ' + S + r' \}', head):
    keys += [m.group(1), m.group(2)]
tools = re.search(r'pub const TOOLS: &\[ToolDef\] = &\[(.*?)\n\];', head, re.S).group(1)
keys += [m.group(1) for m in re.finditer(r'(?:name|hint): ' + S, tools)]
nav = re.search(r'const NAV_MODES: \[\(&str, &str\); 3\] = \[(.*?)\];', head, re.S).group(1)
keys += [m.group(1) for m in re.finditer(S, nav)]
for m in re.finditer(r'section\(ui, ' + S + r'\)', head):
    keys.append(m.group(1))
for m in re.finditer(r'tr\(if [^{]*\{\s*' + S + r'\s*\} else \{\s*' + S + r'\s*\}\)', head, re.S):
    keys += [m.group(1), m.group(2)]
m = re.search(r'const TOUR: \[\(&str, &str\); \d+\] = \[(.*?)\];', head, re.S)
keys += [k.group(1) for k in re.finditer(S, m.group(1))]
for const in ['ORIGINS', 'TRAVEL', 'START_FROM']:
    m = re.search(const + r': \[&str; \d+\] = \[(.*?)\];', head, re.S)
    keys += [k.group(1) for k in re.finditer(S, m.group(1))]
m = re.search(r'pub const TOPICS: &\[guide::Topic\] = &\[(.*?)\n\];', head, re.S)
ts = [k.group(1) for k in re.finditer(S, m.group(1))]
keys += [t for i, t in enumerate(ts) if i % 3 < 2]  # question, answer (not the command)
m = re.search(r'fn mode_name\(m: Mode\) -> &.static str \{(.*?)\n\}', head, re.S)
keys += [k.group(1) for k in re.finditer(S, m.group(1))]
raster = open(os.path.join(base, '..', '..', 'gburn-core', 'src', 'raster.rs')).read()
m = re.search(r'pub const ALL: \[\(Dither, &.static str\); \d+\] = \[(.*?)\];', raster, re.S)
keys += [k.group(1) for k in re.finditer(S, m.group(1))]
look = open(os.path.join(base, 'look.rs')).read()
keys += [k.group(1) for k in re.finditer(r'name: ' + S, look)]
keys += ["Scale X / Y", "Bulge X / Y", "Trapezoid X / Y", "× size", "+ sides bow out", "+ wider top / taller right", "Cuts", "Properties", "Laser"]
seen, out = set(), []
for k in keys:
    k = k.replace('\\"', '"').replace('\\\\', '\\')
    if k in seen or not re.search(r'[A-Za-z]', k):
        continue
    seen.add(k)
    out.append(k.replace('\n', '\\n'))
print('\n'.join(out))
