"""Open, edit, save and render the Word and PowerPoint files of the corpus with the installed oxoffice package, on
the main thread, then measure the stack the same work needs.

The release workflow runs it on each wheel it built, on a few files (--files), before the GitHub release, PyPI and
prefix.dev; the full checks (full-checks.yml) run it on the whole corpus of a release-profile Linux wheel, with
--measure. It stays off pytest on purpose: the Python tests run each test on an 8 MiB thread
(crates/oxoffice-py/tests/conftest.py), while a user's script runs on the main thread. A stack overflow ends the
process without a traceback.

    python -I scripts/wheel_smoke.py tests/corpus/files [--files word/paper.docx ...] [--version X.Y.Z] [--measure]

With --measure, it then finds the smallest thread stack on which the same work completes, by running it in new
processes on threads of decreasing stack sizes, and fails above STACK_BUDGET_KIB.
"""

import argparse
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from pathlib import Path

import oxoffice
import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx

# The stack the work must fit in: 1 MiB is what the Windows linker gives a program's main thread by default, so most
# Windows programs that embed Python run it with that much. python.exe itself reserves 2,000,000 bytes, Linux and
# macOS main threads 8 MiB.
STACK_BUDGET_KIB = 1024

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


# The files of --files, relative to the corpus folder; every file of the corpus when empty.
ONLY = []


def corpus_files(corpus):
    files = ([corpus / name for name in ONLY] if ONLY
             else sorted(corpus.glob("*/*.docx")) + sorted(corpus.glob("*/*.pptx")))
    assert files and all(f.is_file() for f in files), f"missing corpus files in {corpus}: {files}"
    return files


def work(corpus, quiet=False):
    with tempfile.TemporaryDirectory() as tmp:
        for i, source in enumerate(corpus_files(corpus)):
            start = time.perf_counter()
            (document if source.suffix == ".docx" else presentation)(source, Path(tmp) / f"{i}{source.suffix}")
            if not quiet:
                print(f"ok {source.relative_to(corpus)} {time.perf_counter() - start:.2f} s", flush=True)


def on_thread(corpus, kib):
    """Exit 0 when the work completes on a new thread with `kib` KiB of stack. An overflow kills the process."""
    raised = []

    def run():
        try:
            work(corpus, quiet=True)
        except BaseException as error:  # noqa: BLE001 - reported by the exit status
            raised.append(error)
            traceback.print_exception(error)

    if sys.platform == "win32":
        run_on_windows_thread(run, kib * 1024)
    else:
        threading.stack_size(kib * 1024)
        thread = threading.Thread(target=run)
        thread.start()
        thread.join()
    return 1 if raised else 0


def run_on_windows_thread(run, size):
    """Run `run` on a native thread whose stack reserve is `size` bytes.

    threading.stack_size hands _beginthreadex a commit size, and Windows then keeps the executable's larger default
    reserve, so a thread made that way never has less than python.exe's 2,000,000 bytes."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    routine = ctypes.WINFUNCTYPE(wintypes.DWORD, wintypes.LPVOID)
    kernel32.CreateThread.argtypes = [wintypes.LPVOID, ctypes.c_size_t, routine, wintypes.LPVOID, wintypes.DWORD,
                                      wintypes.LPVOID]
    kernel32.CreateThread.restype = wintypes.HANDLE
    kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    stack_size_param_is_a_reservation, infinite = 0x00010000, 0xFFFFFFFF

    @routine
    def start(_):
        run()
        return 0

    handle = kernel32.CreateThread(None, size, start, None, stack_size_param_is_a_reservation, None)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    kernel32.WaitForSingleObject(handle, infinite)
    kernel32.CloseHandle(handle)


def fits(corpus, kib):
    """Whether the work completes on a `kib` KiB thread stack, in a new process. A stack overflow kills that process;
    an exit status of 1 is a Python error, which stops the measure with its traceback."""
    # -X utf8 makes the probe write UTF-8 even on Windows; -I ignores PYTHONIOENCODING.
    command = [sys.executable, "-I", "-X", "utf8", __file__, str(corpus), "--thread-stack", str(kib),
               *(["--files", *ONLY] if ONLY else [])]
    probe = subprocess.run(command, capture_output=True, encoding="utf-8", errors="replace")
    if probe.returncode == 1:
        raise SystemExit(f"the probe on a {kib} KiB stack failed with an error, not an overflow:\n{probe.stderr}")
    return probe.returncode == 0


def stack_needed(corpus, low=128, high=8192):
    """The smallest thread stack on which the work completes, or None above `high`, in steps of 16 KiB, or 64 KiB on
    Windows, which rounds a stack reserve up to that.

    `low` is the smallest stack every platform accepts (glibc on aarch64 refuses less than 128 KiB). The work must
    fail on it: if it does not, either the thread did not get the stack it asked for, and any number would be wrong,
    or the work now needs less, and `low` can go down where the platform allows it."""
    step = 64 if sys.platform == "win32" else 16
    if fits(corpus, low):
        raise SystemExit(f"the work completed on a {low} KiB thread stack, the smallest this measure tries: "
                         "either the stack size is not honoured here, or the work needs less than that")
    if not fits(corpus, high):
        return None
    while high - low > step:
        middle = (low + high) // 2 // step * step
        low, high = (low, middle) if fits(corpus, middle) else (middle, high)
    return high


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--files", nargs="+", default=[], metavar="NAME", help="only these files of the corpus")
    parser.add_argument("--version", help="the version the installed package must report")
    parser.add_argument("--measure", action="store_true", help="also report the smallest thread stack the work needs")
    parser.add_argument("--thread-stack", type=int, metavar="KIB", help=argparse.SUPPRESS)
    args = parser.parse_args()
    ONLY[:] = args.files
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
        need = f"{kib} KiB" if kib else "more than 8 MiB"
        line = f"the same work needs a thread stack of {need}, budget {STACK_BUDGET_KIB} KiB"
        print(line)
        if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(summary, "a", encoding="utf-8") as f:
                f.write(f"{sys.platform} {platform.machine()}: {line}\n")
        if not kib or kib > STACK_BUDGET_KIB:
            sys.exit(f"over the stack budget of {STACK_BUDGET_KIB} KiB")


if __name__ == "__main__":
    sys.exit(main())
