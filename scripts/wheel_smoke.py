"""Open, edit, save and render every Word and PowerPoint file of the corpus with the installed oxoffice package, on
the main thread.

The release workflow runs it on each wheel it built, before publishing anything. It stays off pytest on purpose:
the Python tests run each test on an 8 MiB thread (crates/oxoffice-py/tests/conftest.py), while a user's script runs
on the main thread: 2,000,000 bytes under python.exe on Windows, 1 MiB in most Windows programs that embed Python. A
stack overflow ends the process without a traceback.

    python -I scripts/wheel_smoke.py tests/corpus/files [--version X.Y.Z] [--measure]

With --measure, it then reports the smallest thread stack on which the same work completes, found by running it in
new processes on threads of decreasing stack sizes.
"""

import argparse
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import oxoffice
import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx

MARK = "Added by the oxoffice wheel smoke test"
REPLACED = "Replaced by the oxoffice wheel smoke test"


def document(source, out):
    doc = oxdocx.Document(source)
    assert any(p.text.strip() for p in doc.paragraphs), "no text"
    doc.add_paragraph(MARK)
    doc.add_table(2, 2)
    doc.core_properties.title = MARK
    assert doc.replace_all([(MARK, REPLACED, 1)]) == (1,)
    doc.save(out)
    saved = oxdocx.Document(out)
    assert any(p.text == REPLACED for p in saved.paragraphs), "edit lost on save"
    assert saved.to_pdf().startswith(b"%PDF"), "not a PDF"


def presentation(source, out):
    prs = oxpptx.Presentation(source)
    count = len(prs.slides)
    assert any(shape.has_text_frame and shape.text_frame.text.strip() for slide in prs.slides for shape in slide.shapes)
    slide = prs.slides.add_slide(prs.slide_layouts[len(prs.slide_layouts) - 1])
    inch = oxpptx.Inches(1)
    slide.shapes.add_textbox(inch, inch, 4 * inch, inch).text_frame.text = MARK
    copy = prs.slides.duplicate(prs.slides[0])
    prs.slides.remove(copy)
    prs.core_properties.title = MARK
    assert prs.replace_all([(MARK, REPLACED, 1)]) == (1,)
    prs.save(out)
    saved = oxpptx.Presentation(out)
    assert len(saved.slides) == count + 1, "slide count"
    assert any(s.has_text_frame and s.text_frame.text == REPLACED for s in saved.slides[-1].shapes), "edit lost on save"
    assert saved.to_pdf().startswith(b"%PDF"), "not a PDF"


def corpus_files(corpus):
    files = sorted(corpus.glob("*/*.docx")) + sorted(corpus.glob("*/*.pptx"))
    assert files, f"no corpus file in {corpus}"
    return files


def work(corpus, quiet=False):
    with tempfile.TemporaryDirectory() as tmp:
        for i, source in enumerate(corpus_files(corpus)):
            start = time.perf_counter()
            (document if source.suffix == ".docx" else presentation)(source, Path(tmp) / f"{i}{source.suffix}")
            if not quiet:
                print(f"ok {source.relative_to(corpus)} {time.perf_counter() - start:.2f} s", flush=True)


def on_thread(corpus, kib):
    """Exit 0 when the work completes on a thread of `kib` KiB of stack. An overflow kills the process."""
    raised = []

    def run():
        try:
            work(corpus, quiet=True)
        except BaseException as error:  # noqa: BLE001 - reported by the exit status
            raised.append(error)

    threading.stack_size(kib * 1024)
    thread = threading.Thread(target=run)
    thread.start()
    thread.join()
    return 1 if raised else 0


def fits(corpus, kib):
    command = [sys.executable, "-I", __file__, str(corpus), "--thread-stack", str(kib)]
    return subprocess.run(command, capture_output=True).returncode == 0


def stack_needed(corpus, low=64, high=8192, step=16):
    """The smallest thread stack, to `step` KiB, on which the work completes, or None above `high`."""
    if not fits(corpus, high):
        return None
    while high - low > step:
        middle = (low + high) // 2 // step * step
        low, high = (low, middle) if fits(corpus, middle) else (middle, high)
    return high


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--version", help="the version the installed package must report")
    parser.add_argument("--measure", action="store_true", help="also report the smallest thread stack the work needs")
    parser.add_argument("--thread-stack", type=int, metavar="KIB", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.thread_stack:
        return on_thread(args.corpus, args.thread_stack)
    assert threading.current_thread() is threading.main_thread()
    if args.version:
        assert oxoffice.__version__ == args.version, f"installed {oxoffice.__version__}, expected {args.version}"
    command = subprocess.run(["oxoffice", "--version"], capture_output=True, text=True, check=True).stdout.strip()
    print(f"oxoffice {oxoffice.__version__} from {Path(oxoffice.__file__).parent}, command: {command}")
    work(args.corpus)
    print(f"{len(corpus_files(args.corpus))} files opened, edited, saved, reopened and rendered on the main thread")
    if args.measure:
        kib = stack_needed(args.corpus)
        line = f"the same work needs a thread stack of {kib} KiB" if kib else "the work needs more than 8 MiB of stack"
        print(line)
        if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(summary, "a", encoding="utf-8") as f:
                f.write(f"{sys.platform} {platform.machine()}: {line}\n")


if __name__ == "__main__":
    sys.exit(main())
