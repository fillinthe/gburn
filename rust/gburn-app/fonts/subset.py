# Cut Noto Sans JP / SC (variable, OFL) down to the glyphs GBurn's Japanese / Chinese tables use.
# usage: python3 -I fonts/subset.py ja=NotoSansJP[wght].ttf zh=NotoSansSC[wght].ttf ko=NotoSansKR[wght].ttf  (any of them)   (needs fonttools: pip install fonttools)
import subprocess, sys, os
here = os.path.dirname(os.path.abspath(__file__))
lang = os.path.join(here, '..', 'src', 'lang')
ascii_ = ''.join(chr(c) for c in range(0x20, 0x7f))
def cut(src, text, out):
    subprocess.run([sys.executable, '-m', 'fontTools.subset', src, '--text=' + text, '--output-file=' + os.path.join(here, out),
                    '--layout-features=*', '--no-hinting', '--desubroutinize'], check=True)
NAMES = {'ja': '日本語', 'zh': '简体中文', 'ko': '한국어'}
for code, src in (a.split('=', 1) for a in sys.argv[1:]):
    name = NAMES[code]
    text = ''.join(sorted(set(open(os.path.join(lang, code + '.txt'), encoding='utf-8').read()) - set('\n\t'))) + ascii_
    cut(src, text, 'cjk-%s.ttf' % code)
    cut(src, name, 'names-%s.ttf' % code)
    n = os.path.join(here, 'names-%s.ttf' % code)  # no tweak is set on these: pin them at Regular
    subprocess.run([sys.executable, '-m', 'fontTools.varLib.instancer', n, 'wght=400', '-o', n], check=True)
