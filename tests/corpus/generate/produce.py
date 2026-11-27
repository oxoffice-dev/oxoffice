"""Open and save the authored corpus files with real producers.

    python3 tests/corpus/generate/produce.py [word] [powerpoint] [excel] [libreoffice] [google] [word-google] [--only family ...]
    python3 tests/corpus/generate/produce.py --neutral-excel <file.xlsx> ...

Each producer reads files/authored/* and writes files/<producer>/ with the
same file names, then the manifest is rewritten: the files this run wrote get
the producer's version, the others keep theirs.
Every producer by default, every family but the pilot (report, deck) unless
--only names some: the behaviour tests pin facts of the pilot's copies.

- word, powerpoint, excel: Microsoft Word, PowerPoint and Excel for Mac through
  AppleScript. The apps are sandboxed, so files go through their container
  folder. Each document is closed right after its save, the app quits at the
  end even when a file fails, and a file that takes more than five minutes (a
  dialog waiting for an answer) stops the run. Every workbook Excel saves is
  made neutral right after (neutral_excel): `--neutral-excel` does the same to
  a workbook made by hand in Excel.
- libreoffice: `soffice --headless`, on any machine.
- google: an rclone remote (OXOFFICE_CORPUS_RCLONE, default `gdrive:`) uploads
  with conversion to Google Docs and Slides, exports back, and removes the
  temporary folder. `word-google` does the same with the Word copies of the
  families co-edited in Word and Google Docs (WORD_GOOGLE, those of them
  --only names when it names some), into files/word-google/: a file edited in Word, then in Google Docs;
  the plain `google` route leaves those families out. Embedded faces whose licence is not the SIL Open Font
  License are dropped from the Docs exports (drop_unlicensed_fonts); Slides
  exports embed no font. The Google Sheets copies (files/google/*.xlsx) are
  not made here: they are uploaded through the Google Drive connector with
  conversion to Google Sheets and exported as xlsx (see the README).

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
import neutral_xlsx

FILES = Path(__file__).resolve().parent.parent / "files"
AUTHORED = FILES / "authored"
TIMEOUT = 300
CONTAINERS = {
    "word": Path.home() / "Library/Containers/com.microsoft.Word/Data/oxoffice-corpus",
    "powerpoint": Path.home() / "Library/Containers/com.microsoft.Powerpoint/Data/oxoffice-corpus",
    "excel": Path.home() / "Library/Containers/com.microsoft.Excel/Data/oxoffice-corpus",
}
APPLICATIONS = {"word": "Microsoft Word", "powerpoint": "Microsoft PowerPoint", "excel": "Microsoft Excel"}
EXTENSIONS = {"word": "docx", "powerpoint": "pptx", "excel": "xlsx"}
# The name of Excel's Open XML workbook format in its AppleScript dictionary
# (`sdef /Applications/Microsoft\ Excel.app | grep -i xml` lists the names).
EXCEL_XLSX_FORMAT = "Excel XML file format"
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
    # Excel takes HFS paths, made outside its tell block (`POSIX file` inside it fails with -50), and its
    # dictionary has a `message` property, so the error variables have other names.
    "excel": f"""on run argv
  set sourcePath to (POSIX file (item 1 of argv)) as text
  set targetPath to (POSIX file (item 2 of argv)) as text
  with timeout of 600 seconds
    tell application "Microsoft Excel"
      try
        open workbook workbook file name sourcePath
        save workbook as active workbook filename targetPath file format {EXCEL_XLSX_FORMAT}
      on error errorText number errorNumber
        try
          close active workbook saving no
        end try
        error errorText number errorNumber
      end try
      close active workbook saving no
    end tell
  end timeout
end run""",
}
QUIT_SCRIPTS = {
    "word": 'with timeout of 120 seconds\ntell application "Microsoft Word" to quit saving no\nend timeout',
    "powerpoint": 'with timeout of 120 seconds\ntell application "Microsoft PowerPoint" to quit saving no\nend timeout',
    "excel": 'with timeout of 120 seconds\ntell application "Microsoft Excel" to quit saving no\nend timeout',
}


# What Excel for Mac writes about the machine and the person, replaced in every workbook it saves.
EXCEL_HOME = neutral_xlsx.HOME
ABS_PATH = re.compile(r'(<x15ac:absPath\b[^>]*?\burl=")[^"]*(")')


def neutral_excel(path: Path) -> list[str]:
    """Make a workbook saved by Excel for Mac neutral, and check it: the folder it was saved from (`x15ac:absPath`
    in xl/workbook.xml, kept with the url EXCEL_HOME), the account of the persons part, the creator and the last
    editor of the core properties and the authors of the notes (neutral_xlsx), and the task pane add-ins it
    carries (xl/webextensions/, with their relationships and content types) are replaced or dropped, no other
    byte changes. The result is read again: it raises NotNeutral, and writes nothing, when an account name, an
    email, a path or an add-in is left, or a pattern does not match what the workbook writes. Returns the parts
    that changed."""
    infos, parts = neutral_xlsx.read(path)
    before = dict(parts)
    names = neutral_xlsx.neutral_parts(parts, [])
    name = "xl/workbook.xml"
    if name in parts:
        text = neutral_xlsx.replaced(
            parts[name].decode("utf-8"), "x15ac:absPath", ABS_PATH, rf"\g<1>{EXCEL_HOME}\g<2>", name
        )
        parts[name] = text.encode("utf-8")
    for name in [name for name in parts if name.startswith("xl/webextensions/")]:
        del parts[name]
    for name in parts:
        if name.endswith(".rels"):
            text = parts[name].decode("utf-8")
            parts[name] = re.sub(r"<Relationship\b[^>]*webextension[^>]*/>", "", text).encode("utf-8")
        elif name == "[Content_Types].xml":
            text = parts[name].decode("utf-8")
            parts[name] = re.sub(r'<Override PartName="/xl/webextensions/[^"]*"[^>]*/>', "", text).encode("utf-8")
    left = [name for name, data in parts.items() if name.endswith((".xml", ".rels")) and b"webextension" in data]
    if left:
        raise neutral_xlsx.NotNeutral(f"add-in references left in {left}")
    neutral_xlsx.check(parts, names)
    if parts != before:
        neutral_xlsx.write(path, infos, parts)
    return sorted(name for name in before if parts.get(name) != before[name])


def osascript(script: str, *args: str) -> str:
    return subprocess.run(
        ["osascript", "-e", script, *args], check=True, capture_output=True, text=True, timeout=TIMEOUT
    ).stdout.strip()


# The pilot families: the behaviour tests pin facts of every producer's copy,
# so a run saves them again only when --only names them.
PILOT = {"report", "deck"}
# Families that reach Google Docs through their Word copy, never from their
# source: the word-google route only.
WORD_GOOGLE = {"record"}


def word_google_families(only: list[str]) -> list[str]:
    """The families the word-google route takes: WORD_GOOGLE, or those of them --only names."""
    return [family for family in only if family in WORD_GOOGLE] if only else sorted(WORD_GOOGLE)


def sources(extension: str, only: list[str], folder: Path = AUTHORED) -> list[Path]:
    chosen = (lambda stem: stem in only) if only else (lambda stem: stem not in PILOT)
    return [p for p in sorted(folder.glob(f"*.{extension}")) if chosen(p.stem)]


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
                if app == "excel":
                    # `saved` is in the app's container: a copy that is not neutral never reaches files/.
                    neutral_excel(saved)
                shutil.copy(saved, out / source.name)
                print(f"{app}: {source.name}")
            except subprocess.CalledProcessError as error:
                failures.append(f"{source.name}: {error.stderr.strip()}")
                print(f"{app}: {source.name} FAILED: {error.stderr.strip()}")
            except (neutral_xlsx.NotNeutral, zipfile.BadZipFile, UnicodeDecodeError) as error:
                failures.append(f"{source.name}: not neutral, not kept: {error}")
                print(f"{app}: {source.name} FAILED, not kept: {error}")
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
    return {(app, extension): f"{application} for Mac {version}"} if version else {}


def through_libreoffice(only: list[str]) -> dict:
    out = FILES / "libreoffice"
    out.mkdir(exist_ok=True)
    filters = {
        "docx": "docx:MS Word 2007 XML",
        "pptx": "pptx:Impress MS PowerPoint 2007 XML",
        "xlsx": "xlsx:Calc MS Excel 2007 XML",
    }
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
    return {("libreoffice", extension): name for extension in filters}


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


def through_google(only: list[str], route: str = "google") -> dict:
    """Upload with conversion, export back, remove the temporary folder. The `word-google` route uploads the
    Word copies (files/word/*.docx) instead of the authored sources, as a file edited in Word then opened in
    Google Docs, for the WORD_GOOGLE families (those of them --only names); the plain route leaves them out."""
    remote = os.environ.get("OXOFFICE_CORPUS_RCLONE", "gdrive:") + f"oxoffice-corpus-{uuid.uuid4().hex[:8]}"
    out = FILES / route
    out.mkdir(exist_ok=True)
    if route == "word-google":
        families = word_google_families(only)
        chosen = sources("docx", families, FILES / "word") if families else []
    else:
        chosen = [p for p in sources("docx", only) + sources("pptx", only) if p.stem not in WORD_GOOGLE]
    with tempfile.TemporaryDirectory() as staging, tempfile.TemporaryDirectory() as exported:
        for source in chosen:
            shutil.copy(source, staging)
            (out / source.name).unlink(missing_ok=True)
        try:
            subprocess.run(["rclone", "copy", staging, remote, "--drive-import-formats", "docx,pptx"], check=True)
            subprocess.run(["rclone", "copy", remote, exported, "--drive-export-formats", "docx,pptx"], check=True)
            for path in sorted(Path(exported).iterdir()):
                shutil.copy(path, out / path.name)
                dropped = drop_unlicensed_fonts(out / path.name) if path.suffix == ".docx" else []
                print(f"{route}: {path.name}" + (f" (dropped {', '.join(dropped)})" if dropped else ""))
        finally:
            subprocess.run(["rclone", "purge", remote], check=False)
    if route == "word-google":
        return {("word-google", "docx"): "Google Docs export of the Word copy"}
    return {("google", "docx"): "Google Docs export", ("google", "pptx"): "Google Slides export"}


def main() -> None:
    args = sys.argv[1:]
    if args[:1] == ["--neutral-excel"]:
        files = args[1:] or sys.exit(__doc__)
        for name in files:
            print(f"{name}: " + (", ".join(neutral_excel(Path(name))) or "nothing to replace"))
        return
    only = args[args.index("--only") + 1:] if "--only" in args else []
    producers = (args[: args.index("--only")] if "--only" in args else args) or [
        "word", "powerpoint", "excel", "libreoffice", "google", "word-google"
    ]
    versions, started = {}, time.time()
    for producer in producers:
        if producer in ("word", "powerpoint", "excel"):
            versions |= through_office(producer, only)
        elif producer == "libreoffice":
            versions |= through_libreoffice(only)
        elif producer in ("google", "word-google"):
            versions |= through_google(only, producer)
        else:
            sys.exit(f"unknown producer {producer}")
    manifest.update(versions, since=started)


if __name__ == "__main__":
    main()
