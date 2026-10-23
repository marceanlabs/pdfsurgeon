#!/usr/bin/env python3
"""Check that interactive forms survive pdfsurgeon.

Builds two filled-in-capable forms with reportlab, runs the binary over them,
and reads the results back with pypdf, which knows nothing about this project.

    pip install pypdf reportlab
    ./scripts/check_forms.py            # uses ./target/release/pdfsurgeon

Exits non-zero and prints what went wrong when a field is lost, a name is not
made unique, a widget loses its page, or a value cannot be set.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

from reportlab.lib.pagesizes import A4
from reportlab.pdfgen import canvas
from pypdf import PdfReader, PdfWriter

BINARY = os.environ.get("BINARY", "./target/release/pdfsurgeon")


def form(path, second_page_field=True):
    c = canvas.Canvas(str(path), pagesize=A4)
    c.setFont("Helvetica", 16)
    c.drawString(72, 780, "Form")
    c.acroForm.textfield(name="name", x=72, y=700, width=250, height=20,
                         borderStyle="inset", forceBorder=True)
    c.acroForm.checkbox(name="agree", x=72, y=660, size=18, buttonStyle="check",
                        forceBorder=True)
    c.showPage()
    if second_page_field:
        c.setFont("Helvetica", 16)
        c.drawString(72, 780, "Page two")
        c.acroForm.textfield(name="notes", x=72, y=700, width=400, height=60,
                             borderStyle="inset", forceBorder=True)
        c.showPage()
    c.save()


def run(*args):
    result = subprocess.run([BINARY, *map(str, args)], capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"{' '.join(map(str, args))} failed: {result.stderr.strip()}")


failures = []


def check(condition, message):
    if not condition:
        failures.append(message)


def fields(path):
    return PdfReader(str(path)).get_fields() or {}


def fill(path, values):
    reader = PdfReader(str(path))
    writer = PdfWriter()
    writer.append(reader)
    for page in writer.pages:
        writer.update_page_form_field_values(page, values, auto_regenerate=False)
    writer.write(str(path))
    return {name: value.get("/V") for name, value in fields(path).items()}


with tempfile.TemporaryDirectory() as work:
    work = Path(work)
    a, b = work / "a.pdf", work / "b.pdf"
    form(a)
    form(b, second_page_field=False)

    # Merge: both forms keep their fields, and the clash on "name" is renamed.
    merged = work / "merged.pdf"
    run("merge", "-o", merged, a, b)
    names = set(fields(merged))
    check(names == {"name", "name_2", "agree", "agree_2", "notes"},
          f"merge lost or misnamed fields: {sorted(names)}")

    values = fill(merged, {"name": "Adam", "agree": "/Yes", "notes": "page two",
                           "name_2": "second", "agree_2": "/Yes"})
    check(values.get("name") == "Adam", f"name did not take a value: {values}")
    check(values.get("notes") == "page two", f"notes did not take a value: {values}")
    check(values.get("name_2") == "second", f"renamed field did not take a value: {values}")
    check(values.get("agree") == "/Yes", f"checkbox did not take a value: {values}")

    # Select: only the fields whose widget is on the kept page.
    picked = work / "picked.pdf"
    run("select", a, "--pages", "1", "-o", picked)
    check(set(fields(picked)) == {"name", "agree"},
          f"select kept the wrong fields: {sorted(fields(picked))}")

    # Split: every page file carries the field that belongs to it.
    parts = work / "parts"
    run("split", a, "--out", parts)
    check(set(fields(parts / "a-001.pdf")) == {"name", "agree"},
          "split page one lost its fields")
    check(set(fields(parts / "a-002.pdf")) == {"notes"},
          "split page two lost its field")

    # Assemble: pages from both files, names made unique again.
    assembled = work / "assembled.pdf"
    run("assemble", "-o", assembled, f"{a}:1", f"{b}:1")
    check(set(fields(assembled)) == {"name", "name_2", "agree", "agree_2"},
          f"assemble kept the wrong fields: {sorted(fields(assembled))}")

    # The form has to point its widgets at the pages they now sit on.
    reader = PdfReader(str(merged))
    page_ids = {page.indirect_reference.idnum: number for number, page in enumerate(reader.pages, 1)}
    for reference in reader.trailer["/Root"]["/AcroForm"].get_object()["/Fields"]:
        field = reference.get_object()
        page = field.get("/P")
        check(page is not None and page.idnum in page_ids,
              f"field {field.get('/T')} is not attached to a page in the output")
        check("/AA" not in field, f"field {field.get('/T')} kept its JavaScript")

if failures:
    print("form checks failed:")
    for failure in failures:
        print(f"  - {failure}")
    sys.exit(1)

print("forms: merge, select, split and assemble all keep fields that can be filled in")
