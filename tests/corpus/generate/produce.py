"""Open and save the authored corpus files with real producers.

    python3 tests/corpus/generate/produce.py [word] [powerpoint] [libreoffice] [google] [--only family ...]

Each producer reads files/authored/* and writes files/<producer>/ with the
same file names, then the manifest is rewritten: the files this run wrote get
the producer's version, the others keep theirs.
Every producer by default, every family but the pilot (report, deck) unless
--only names some: the behaviour tests pin facts of the pilot's copies.

- word, powerpoint: Microsoft Word and PowerPoint for Mac through AppleScript.
  The apps are sandboxed, so files go through their container folder. Each
  document is closed right after its save, the app quits at the end even when
  a file fails, and a file that takes more than five minutes (a dialog waiting
  for an answer) stops the run.
- libreoffice: `soffice --headless`, on any machine.
- google: an rclone remote (OXOFFICE_CORPUS_RCLONE, default `gdrive:`) uploads
  with conversion to Google Docs and Slides, exports back, and removes the
  temporary folder. Embedded faces whose licence is not the SIL Open Font
  License are dropped from the Docs exports (drop_unlicensed_fonts); Slides
  exports embed no font.

Standard library only, Python 3.11 or later. On the Mac:

    uv run --no-project --python 3.12 tests/corpus/generate/produce.py word powerpoint
"""

import os
import shutil
import subprocess
import sys
import re
import struct
import tempfile
import time
import uuid
import zipfile
from pathlib import Path

import manifest

FILES = Path(__file__).resolve().parent.parent / "files"
AUTHORED = FILES / "authored"
TIMEOUT = 300
CONTAINERS = {
    "word": Path.home() / "Library/Containers/com.microsoft.Word/Data/oxoffice-corpus",
    "powerpoint": Path.home() / "Library/Containers/com.microsoft.Powerpoint/Data/oxoffice-corpus",
}
APPLICATIONS = {"word": "Microsoft Word", "powerpoint": "Microsoft PowerPoint"}
EXTENSIONS = {"word": "docx", "powerpoint": "pptx"}
# Open, save as Open XML, close without saving again. The close runs even when
# the open or the save fails, so that no document stays open. AppleScript waits
# up to 600 s for the app, longer than TIMEOUT, so that a dialog waiting for an
# answer reaches Python's timeout, which stops the run.
SAVE_SCRIPTS = {
    "word": """on run argv
  with timeout of 600 seconds
    tell application "Microsoft Word"
      try
        open file name (item 1 of argv)
        save as active document file name (item 2 of argv) file format format document
      on error message number code
        try
          close active document saving no
        end try
        error message number code
      end try
      close active document saving no
    end tell
  end timeout
end run""",
    "powerpoint": """on run argv
  with timeout of 600 seconds
    tell application "Microsoft PowerPoint"
      try
        open POSIX file (item 1 of argv)
        save active presentation in (POSIX file (item 2 of argv)) as save as Open XML presentation
      on error message number code
        try
          close active presentation saving no
        end try
        error message number code
      end try
      close active presentation saving no
    end tell
  end timeout
end run""",
}
QUIT_SCRIPTS = {
    "word": 'with timeout of 120 seconds\ntell application "Microsoft Word" to quit saving no\nend timeout',
    "powerpoint": 'with timeout of 120 seconds\ntell application "Microsoft PowerPoint" to quit saving no\nend timeout',
}


def osascript(script: str, *args: str) -> str:
    return subprocess.run(
        ["osascript", "-e", script, *args], check=True, capture_output=True, text=True, timeout=TIMEOUT
    ).stdout.strip()


# The pilot families: the behaviour tests pin facts of every producer's copy,
# so a run saves them again only when --only names them.
PILOT = {"report", "deck"}


def sources(extension: str, only: list[str]) -> list[Path]:
    chosen = (lambda stem: stem in only) if only else (lambda stem: stem not in PILOT)
    return [p for p in sorted(AUTHORED.glob(f"*.{extension}")) if chosen(p.stem)]


def through_office(app: str, only: list[str]) -> dict:
    container, out = CONTAINERS[app], FILES / app
    container.mkdir(parents=True, exist_ok=True)
    out.mkdir(exist_ok=True)
    application, extension = APPLICATIONS[app], EXTENSIONS[app]
    failures = []
    version = None
    try:
        version = osascript(f'version of application "{application}"')
        for source in sources(extension, only):
            staged, saved = container / f"in-{source.name}", container / source.name
            shutil.copy(source, staged)
            saved.unlink(missing_ok=True)
            # A file this run fails on must not keep the copy of an earlier run.
            (out / source.name).unlink(missing_ok=True)
            try:
                osascript(SAVE_SCRIPTS[app], str(staged), str(saved))
                shutil.copy(saved, out / source.name)
                print(f"{app}: {source.name}")
            except subprocess.CalledProcessError as error:
                failures.append(f"{source.name}: {error.stderr.strip()}")
                print(f"{app}: {source.name} FAILED: {error.stderr.strip()}")
            except subprocess.TimeoutExpired:
                failures.append(f"{source.name}: no answer after {TIMEOUT} s, a dialog is probably open")
                print(f"{app}: {source.name} took over {TIMEOUT} s: answer or close the dialog in {application}, "
                      "close the document without saving, then rerun with --only for the remaining families")
                break
            finally:
                staged.unlink(missing_ok=True)
                saved.unlink(missing_ok=True)
    finally:
        try:
            osascript(QUIT_SCRIPTS[app])
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            print(f"{application} did not quit: close its documents without saving and quit it by hand")
    if failures:
        print(f"{app}: {len(failures)} file(s) failed:\n  " + "\n  ".join(failures))
    product = "Word" if app == "word" else "PowerPoint"
    return {(app, extension): f"Microsoft {product} for Mac {version}"} if version else {}


def through_libreoffice(only: list[str]) -> dict:
    out = FILES / "libreoffice"
    out.mkdir(exist_ok=True)
    filters = {"docx": "docx:MS Word 2007 XML", "pptx": "pptx:Impress MS PowerPoint 2007 XML"}
    # A profile of its own, so that a LibreOffice already open on the machine is left alone.
    with tempfile.TemporaryDirectory() as profile:
        for extension, target in filters.items():
            for source in sources(extension, only):
                produced = out / source.name
                produced.unlink(missing_ok=True)
                run = subprocess.run(
                    ["soffice", f"-env:UserInstallation=file://{profile}", "--headless", "--convert-to", target,
                     "--outdir", str(out), str(source)],
                    check=True, capture_output=True, text=True, timeout=TIMEOUT,
                )
                if not produced.exists():
                    sys.exit(f"libreoffice wrote no {produced.name}: {run.stdout}{run.stderr}")
                print(f"libreoffice: {source.name}")
    version = subprocess.run(["soffice", "--version"], capture_output=True, text=True).stdout.split()
    name = f"LibreOffice {version[1]}"
    return {("libreoffice", "docx"): name, ("libreoffice", "pptx"): name}


def font_licence(data: bytes) -> str:
    """The licence description and URL (name ids 13 and 14) of a TrueType or
    OpenType face, empty when it names none."""
    texts = []
    try:
        tables = struct.unpack(">H", data[4:6])[0]
        for index in range(tables):
            tag, _, offset, _ = struct.unpack(">4sIII", data[12 + 16 * index : 28 + 16 * index])
            if tag == b"name":
                break
        else:
            return ""
        _, count, strings = struct.unpack(">HHH", data[offset : offset + 6])
        for index in range(count):
            record = data[offset + 6 + 12 * index : offset + 18 + 12 * index]
            platform, _, _, name_id, length, start = struct.unpack(">HHHHHH", record)
            if name_id in (13, 14):
                raw = data[offset + strings + start : offset + strings + start + length]
                texts.append(raw.decode("utf-16-be", "replace") if platform in (0, 3) else raw.decode("latin-1"))
    except struct.error:
        return ""  # not a well-formed face: no licence we can read
    return " ".join(texts)


def drop_unlicensed_fonts(path: Path) -> list[str]:
    """Remove from a Google Docs export every embedded face whose licence is not
    the SIL Open Font License, with its relationship and its font-table entry:
    Google embeds whole faces it licenses (Cambria Math for equations), which
    are not ours to redistribute. Every other part keeps its bytes. Returns the
    parts removed."""
    with zipfile.ZipFile(path) as package:
        infos = package.infolist()
        parts = {info.filename: package.read(info) for info in infos}
    dropped = [
        name
        for name, data in parts.items()
        if name.startswith("word/fonts/")
        and not name.endswith("/")
        and not re.search(r"Open Font License|/OFL\b", font_licence(data))
    ]
    if not dropped:
        return []
    rels_name, table_name = "word/_rels/fontTable.xml.rels", "word/fontTable.xml"
    rels, table = parts[rels_name].decode(), parts[table_name].decode()
    for name in dropped:
        target = re.escape(name.removeprefix("word/"))
        for relationship in re.findall(rf'<Relationship [^>]*Target="{target}"[^>]*/>', rels):
            rid = re.search(r'Id="([^"]+)"', relationship).group(1)
            rels = rels.replace(relationship, "")
            table = re.sub(rf'<w:embed(?:Regular|Bold|Italic|BoldItalic) [^>]*r:id="{rid}"[^>]*/>', "", table)
        del parts[name]
    parts[rels_name], parts[table_name] = rels.encode(), table.encode()
    with zipfile.ZipFile(path, "w") as package:
        for info in infos:
            if info.filename in parts:
                package.writestr(info, parts[info.filename])
    return dropped


def through_google(only: list[str]) -> dict:
    remote = os.environ.get("OXOFFICE_CORPUS_RCLONE", "gdrive:") + f"oxoffice-corpus-{uuid.uuid4().hex[:8]}"
    out = FILES / "google"
    out.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as staging, tempfile.TemporaryDirectory() as exported:
        for source in sources("docx", only) + sources("pptx", only):
            shutil.copy(source, staging)
            (out / source.name).unlink(missing_ok=True)
        try:
            subprocess.run(["rclone", "copy", staging, remote, "--drive-import-formats", "docx,pptx"], check=True)
            subprocess.run(["rclone", "copy", remote, exported, "--drive-export-formats", "docx,pptx"], check=True)
            for path in sorted(Path(exported).iterdir()):
                shutil.copy(path, out / path.name)
                dropped = drop_unlicensed_fonts(out / path.name) if path.suffix == ".docx" else []
                print(f"google: {path.name}" + (f" (dropped {', '.join(dropped)})" if dropped else ""))
        finally:
            subprocess.run(["rclone", "purge", remote], check=False)
    return {("google", "docx"): "Google Docs export", ("google", "pptx"): "Google Slides export"}


def main() -> None:
    args = sys.argv[1:]
    only = args[args.index("--only") + 1:] if "--only" in args else []
    producers = (args[: args.index("--only")] if "--only" in args else args) or [
        "word", "powerpoint", "libreoffice", "google"
    ]
    versions, started = {}, time.time()
    for producer in producers:
        if producer in ("word", "powerpoint"):
            versions |= through_office(producer, only)
        elif producer == "libreoffice":
            versions |= through_libreoffice(only)
        elif producer == "google":
            versions |= through_google(only)
        else:
            sys.exit(f"unknown producer {producer}")
    manifest.update(versions, since=started)


if __name__ == "__main__":
    main()
