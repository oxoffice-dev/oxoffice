"""Open each probe in Word, update every field, save the result in word/, close it, and quit Word.

cargo run -p oxoffice-docx --example bibliography_probe -- crates/oxoffice-docx/tests/docx/bibliography/probe
uv run --no-project --python 3.12 crates/oxoffice-docx/tests/docx/bibliography/update_in_word.py

Word for Mac renders every probe in the style set in Word (References > Style), not the
probe's own: run it once per style with that style set, and keep only the matching file.
"""

import pathlib
import subprocess

HERE = pathlib.Path(__file__).resolve().parent
TIMEOUT = 300

# The script updates every field, saves as .docx, and closes the document whatever happens.
SCRIPT = """
on run argv
    set source to POSIX file (item 1 of argv)
    set target to item 2 of argv
    tell application "Microsoft Word"
        open source
        set probe to active document
        try
            -- Word 16.115 has no "update fields" on a text object; update each field instead.
            repeat with f in (fields of probe)
                update field f
            end repeat
            save as probe file name target file format format document
        on error message
            close probe saving no
            error message
        end try
        close probe saving no
    end tell
end run
"""

def main() -> None:
    (HERE / "word").mkdir(exist_ok=True)
    try:
        for probe in sorted((HERE / "probe").glob("*.docx")):
            target = HERE / "word" / probe.name
            subprocess.run(
                ["osascript", "-e", SCRIPT, str(probe), str(target)],
                check=True, capture_output=True, text=True, timeout=TIMEOUT,
            )
            print(target)
    finally:
        subprocess.run(["osascript", "-e", 'tell application "Microsoft Word" to quit saving no'], check=False)

if __name__ == "__main__":
    main()
