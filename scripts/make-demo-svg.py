#!/usr/bin/env python3
"""Turn captured terminal output into an SVG terminal window.

    scripts/make-demo-svg.py captured.txt assets/demo.svg

The text must be what the program actually printed; nothing here invents output.
"""

import sys
from xml.sax.saxutils import escape

WIDTH = 780
LINE_HEIGHT = 22
PADDING = 20
BAR = 36


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    source, target = sys.argv[1], sys.argv[2]
    with open(source, encoding="utf-8") as handle:
        lines = handle.read().rstrip("\n").split("\n")

    height = BAR + PADDING * 2 + LINE_HEIGHT * len(lines)
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
        f'viewBox="0 0 {WIDTH} {height}" font-family="ui-monospace, SF Mono, Menlo, '
        f'Consolas, monospace">',
        '<defs><clipPath id="r"><rect width="100%" height="100%" rx="8"/></clipPath></defs>',
        f'<g clip-path="url(#r)"><rect width="{WIDTH}" height="{height}" fill="#0d1117"/>',
        f'<rect width="{WIDTH}" height="{BAR}" fill="#161b22"/>',
    ]
    for index, colour in enumerate(("#ff5f57", "#febc2e", "#28c840")):
        out.append(
            f'<circle cx="{20 + index * 20}" cy="{BAR / 2}" r="6" fill="{colour}"/>'
        )
    out.append("</g>")
    y = BAR + PADDING + LINE_HEIGHT - 6
    for line in lines:
        if line.startswith("$ "):
            out.append(
                f'<text x="{PADDING}" y="{y}" font-size="15" fill="#3fb950">$</text>'
            )
            out.append(
                f'<text x="{PADDING + 18}" y="{y}" font-size="15" fill="#e6edf3">'
                f"{escape(line[2:])}</text>"
            )
        elif line.startswith(">"):
            out.append(
                f'<text x="{PADDING}" y="{y}" font-size="15" fill="#8b949e">'
                f"{escape(line[1:])}</text>"
            )
        else:
            out.append(
                f'<text x="{PADDING}" y="{y}" font-size="15" fill="#c9d1d9">'
                f"{escape(line)}</text>"
            )
        y += LINE_HEIGHT
    out.append("</svg>")
    with open(target, "w", encoding="utf-8") as handle:
        handle.write("\n".join(out) + "\n")
    print(f"{target}: {len(lines)} lines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
