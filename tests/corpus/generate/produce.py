"""Open and save the authored corpus files with real producers.

    python3 tests/corpus/generate/produce.py [word] [powerpoint] [libreoffice] [google]

Each producer reads files/authored/* and writes files/<producer>/ with the
same file names:

- word, powerpoint: Microsoft Word and PowerPoint for Mac through AppleScript.
  The apps are sandboxed, so files go through their container folder. Every
  document is closed after its save and the app quits at the end.
- libreoffice: `soffice --headless`.
- google: an rclone remote (OXOFFICE_CORPUS_RCLONE, default `gdrive:`) uploads
  with conversion to Google Docs and Slides, exports back, and removes the
  temporary folder.

Prints the producer versions to record in manifest.toml.
"""

import shutil
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "files"
AUTHORED = ROOT / "authored"
CONTAINERS = {
    "word": Path.home() / "Library/Containers/com.microsoft.Word/Data/oxoffice-corpus",
    "powerpoint": Path.home() / "Library/Containers/com.microsoft.Powerpoint/Data/oxoffice-corpus",
}
WORD_SAVE = """on run argv
  tell application "Microsoft Word"
    open file name (item 1 of argv)
    save as active document file name (item 2 of argv) file format format document
    close active document saving no
  end tell
end run"""
POWERPOINT_SAVE = """on run argv
  tell application "Microsoft PowerPoint"
    open POSIX file (item 1 of argv)
    save active presentation in (POSIX file (item 2 of argv)) as save as Open XML presentation
    close active presentation saving no
  end tell
end run"""


def osascript(script: str, *args: str) -> str:
    return subprocess.run(["osascript", "-e", script, *args], check=True, capture_output=True, text=True).stdout.strip()


def through_office(app: str, extension: str, script: str) -> None:
    container = CONTAINERS[app]
    container.mkdir(parents=True, exist_ok=True)
    out = ROOT / app
    out.mkdir(exist_ok=True)
    application = "Microsoft Word" if app == "word" else "Microsoft PowerPoint"
    try:
        for source in sorted(AUTHORED.glob(f"*.{extension}")):
            staged, saved = container / f"in-{source.name}", container / source.name
            shutil.copy(source, staged)
            saved.unlink(missing_ok=True)
            osascript(script, str(staged), str(saved))
            shutil.copy(saved, out / source.name)
            print(f"{app}: {source.name}")
    finally:
        print(f"{app} version: {osascript(f'version of application \"{application}\"')}")
        osascript(f'tell application "{application}" to quit saving no' if app == "word" else f'tell application "{application}" to quit')


def through_libreoffice() -> None:
    out = ROOT / "libreoffice"
    out.mkdir(exist_ok=True)
    for source in sorted(AUTHORED.iterdir()):
        target = {"docx": 'docx:"MS Word 2007 XML"', "pptx": 'pptx:"Impress MS PowerPoint 2007 XML"'}.get(source.suffix[1:])
        if target:
            subprocess.run(["soffice", "--headless", "--convert-to", target.replace('"', ""), "--outdir", str(out), str(source)],
                           check=True, capture_output=True)
            print(f"libreoffice: {source.name}")
    print(subprocess.run(["soffice", "--version"], capture_output=True, text=True).stdout.strip())


def through_google() -> None:
    import os

    remote = os.environ.get("OXOFFICE_CORPUS_RCLONE", "gdrive:") + f"oxoffice-corpus-{uuid.uuid4().hex[:8]}"
    out = ROOT / "google"
    out.mkdir(exist_ok=True)
    try:
        subprocess.run(["rclone", "copy", str(AUTHORED), remote, "--drive-import-formats", "docx,pptx"], check=True)
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["rclone", "copy", remote, tmp, "--drive-export-formats", "docx,pptx"], check=True)
            for exported in Path(tmp).iterdir():
                shutil.copy(exported, out / exported.name)
                print(f"google: {exported.name}")
    finally:
        subprocess.run(["rclone", "purge", remote], check=False)


def main() -> None:
    producers = sys.argv[1:] or ["word", "powerpoint", "libreoffice", "google"]
    for producer in producers:
        if producer == "word":
            through_office("word", "docx", WORD_SAVE)
        elif producer == "powerpoint":
            through_office("powerpoint", "pptx", POWERPOINT_SAVE)
        elif producer == "libreoffice":
            through_libreoffice()
        elif producer == "google":
            through_google()
        else:
            sys.exit(f"unknown producer {producer}")


if __name__ == "__main__":
    main()
