"""A deck whose part holds a character XML 1.0 cannot carry is refused at open, naming the part, the line and the
column: PowerPoint cannot read that part either, it repairs the deck by dropping what the part holds."""

import zipfile
from pathlib import Path

import pytest

import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


def test_a_part_holding_a_control_character_is_refused_with_where_it_is(tmp_path):
    path = tmp_path / "control.pptx"
    with zipfile.ZipFile(CORPUS / "powerpoint" / "deck.pptx") as source, zipfile.ZipFile(path, "w") as out:
        for info in source.infolist():
            data = source.read(info.filename)
            if info.filename == "ppt/slides/slide1.xml":
                data = data.replace(b"<a:t>", b"<a:t>\x01", 1)
            out.writestr(info, data)

    with pytest.raises(oxpptx.PackageError) as raised:
        oxpptx.Presentation(path)
    message = str(raised.value)
    assert "/ppt/slides/slide1.xml holds U+0001 at line 2, column " in message
    assert "remove the character from the part" in message
