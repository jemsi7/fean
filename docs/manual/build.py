"""Build the manual: figures from the IDE's automatic mode, the feature table from the IDE's own
list, then the PDF (Map unit I-12-a).

    python3 docs/manual/build.py              # figures, feature table, PDF
    python3 docs/manual/build.py --no-shots   # keep the figures; refresh the table and the PDF

Needs dist/fean-ide (bash rust/build.sh), Pillow, and Google Chrome for printing the HTML to PDF.
The figures are the screens the IDE saves in its automatic mode, scaled down for print.
"""
import html
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
IDE = ROOT / 'dist' / 'fean-ide'
SOURCE = HERE / 'manual.html'
PDF = HERE / 'FEAN-IDE-설명서.pdf'
IMAGES = HERE / 'img'
CHROME = Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
WIDTH = 1600


def figures():
    """Run the scenario and keep its screens, scaled to WIDTH pixels and reduced to 256 colours."""
    from PIL import Image
    with tempfile.TemporaryDirectory() as tmp:
        shots = Path(tmp) / 'shots'
        subprocess.run([str(IDE), '--shots', str(shots)], check=True, timeout=300)
        report = json.loads((shots / 'report.json').read_text(encoding='utf-8'))
        if not report['ok']:
            raise SystemExit(f'the scenario failed: {report["error"]}')
        IMAGES.mkdir(exist_ok=True)
        for old in IMAGES.glob('*.png'):
            if not old.name.startswith('._'):       # the volume keeps a sidecar per file; it goes with its file
                old.unlink(missing_ok=True)
        for name in report['shots']:
            with Image.open(shots / name) as image:
                height = round(image.height * WIDTH / image.width)
                small = image.convert('RGB').resize((WIDTH, height), Image.LANCZOS)
                small = small.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
                small.save(IMAGES / name, optimize=True)
        return len(report['shots'])


def feature_table():
    """Write the IDE's own feature list into the manual, between the two markers."""
    rows = json.loads(subprocess.run([str(IDE), '--features'], check=True, capture_output=True, text=True).stdout)
    lines = ['<table>', '  <tr><th>묶음</th><th>기능</th><th>쓰는 법</th></tr>']
    for row in rows:
        cells = (html.escape(row[k]) for k in ('group', 'name', 'how'))
        lines.append('  <tr><td>{}</td><td class="name">{}</td><td>{}</td></tr>'.format(*cells))
    lines.append('</table>')
    text = SOURCE.read_text(encoding='utf-8')
    block = '<!-- features:begin -->\n' + '\n'.join(lines) + '\n<!-- features:end -->'
    text, count = re.subn(r'<!-- features:begin -->.*?<!-- features:end -->', lambda _: block, text, flags=re.S)
    if count != 1:
        raise SystemExit('manual.html has lost its feature markers')
    SOURCE.write_text(text, encoding='utf-8')
    return len(rows)


def pdf():
    """Print the HTML to PDF with headless Chrome (its own empty profile, no header or footer)."""
    missing = [m for m in re.findall(r'src="(img/[^"]+)"', SOURCE.read_text(encoding='utf-8')) if not (HERE / m).exists()]
    if missing:
        raise SystemExit(f'figures are missing: {missing}')
    with tempfile.TemporaryDirectory() as profile:
        out = Path(profile) / 'manual.pdf'
        done = subprocess.run([str(CHROME), '--headless=new', '--disable-gpu', '--no-first-run', '--no-pdf-header-footer',
                               f'--user-data-dir={profile}/profile', '--virtual-time-budget=10000',
                               f'--print-to-pdf={out}', SOURCE.as_uri()],
                              capture_output=True, text=True, timeout=300)
        if not out.exists():
            raise SystemExit(f'Chrome did not write the PDF:\n{done.stderr[-2000:]}')
        shutil.copyfile(out, PDF)
    return PDF.stat().st_size


if __name__ == '__main__':
    if '--no-shots' not in sys.argv[1:]:
        print(f'figures: {figures()}')
    print(f'features: {feature_table()}')
    print(f'pdf: {pdf()} bytes → {PDF.relative_to(ROOT)}')
