"""Core property dates read and write as datetime objects in both packages, with the same answers: a stored W3CDTF
date reads at its right instant in UTC or as None, never at a shifted instant, and a datetime saves as UTC text."""

import datetime
import io
import re
import zipfile

import pytest

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx

UTC = datetime.timezone.utc
FIELDS = ("created", "modified", "last_printed")
TAGS = {"created": "dcterms:created", "modified": "dcterms:modified", "last_printed": "cp:lastPrinted"}


def blank(package):
    return oxdocx.Document() if package is oxdocx else oxpptx.Presentation()


def with_stored_date(package, field, text):
    """A new document or deck whose core.xml stores `text` for `field`, as another producer would write it."""
    doc = blank(package)
    setattr(doc.core_properties, field, datetime.datetime(2000, 1, 1, tzinfo=UTC))
    source = io.BytesIO(doc.to_bytes())
    target = io.BytesIO()
    with zipfile.ZipFile(source) as old, zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as new:
        for item in old.infolist():
            data = old.read(item.filename)
            if item.filename == "docProps/core.xml":
                tag = TAGS[field]
                data, count = re.subn(rf"(<{tag}[^>]*>)[^<]*(</{tag}>)".encode(),
                                      rb"\g<1>" + text.encode() + rb"\g<2>", data)
                assert count == 1
            new.writestr(item, data)
    return type(doc).from_bytes(target.getvalue())


def stored_text(doc, field):
    with zipfile.ZipFile(io.BytesIO(doc.to_bytes())) as package:
        core = package.read("docProps/core.xml").decode()
    match = re.search(rf"<{TAGS[field]}[^>]*>([^<]*)</{TAGS[field]}>", core)
    return match and match.group(1)


BOTH = pytest.mark.parametrize("package", [oxdocx, oxpptx], ids=["docx", "pptx"])

READS = [
    ("2026-11-21T10:00:00Z", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
    ("2026-11-21T10:00:00+02:00", datetime.datetime(2026, 11, 21, 8, tzinfo=UTC)),
    ("2026-11-21T10:00:00-05:30", datetime.datetime(2026, 11, 21, 15, 30, tzinfo=UTC)),
    ("2026-11-21T10:00:00.503Z", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
    ("2026-11-21T10:00:00z", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
    ("2026-11-21T10:00:00Z\n", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
    ("2026-11-21T10:00:00", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
    ("2026-11-21", datetime.datetime(2026, 11, 21, tzinfo=UTC)),
    ("2026-11", datetime.datetime(2026, 11, 1, tzinfo=UTC)),
    ("2026", datetime.datetime(2026, 1, 1, tzinfo=UTC)),
    ("not a date", None),
    ("2026-13-01T00:00:00Z", None),
]


@BOTH
@pytest.mark.parametrize("field", FIELDS)
@pytest.mark.parametrize(("text", "expected"), READS, ids=[text for text, _ in READS])
def test_a_stored_date_reads_as_its_utc_instant_or_none(package, field, text, expected):
    read = getattr(with_stored_date(package, field, text).core_properties, field)
    assert read == expected
    assert read is None or read.utcoffset() == datetime.timedelta(0)


# Where the Word binding's own copy of the conversion still answers differently, until #249 moves it to the shared one.
WORD_DIFFERS = [
    ("2026-11-21T10:00:00.503+02:00", datetime.datetime(2026, 11, 21, 8, tzinfo=UTC)),
    ("2026-11-21T10:00:00+0200", None),
    ("2026-11-21T10:00:00+00:60", None),
    ("2026-11-21T10:00:00.Z", None),
    ("2026-11-21T10:00:00.12345", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
    (" 2026-11-21T10:00:00Z", datetime.datetime(2026, 11, 21, 10, tzinfo=UTC)),
]


@pytest.mark.parametrize("package", [
    pytest.param(oxdocx, marks=pytest.mark.xfail(strict=True, reason="#249: the Word binding reads these at a "
                                                 "shifted instant, or not at all")),
    oxpptx,
], ids=["docx", "pptx"])
@pytest.mark.parametrize("field", FIELDS)
@pytest.mark.parametrize(("text", "expected"), WORD_DIFFERS, ids=[text for text, _ in WORD_DIFFERS])
def test_a_date_is_read_at_its_right_instant_or_not_at_all(package, field, text, expected):
    read = getattr(with_stored_date(package, field, text).core_properties, field)
    assert read == expected
    assert read is None or read.utcoffset() == datetime.timedelta(0)


@BOTH
@pytest.mark.parametrize("field", FIELDS)
def test_naive_and_aware_datetimes_save_as_the_same_utc_text(package, field):
    naive, aware = blank(package), blank(package)
    setattr(naive.core_properties, field, datetime.datetime(2026, 11, 21, 8, 30, 15))
    setattr(aware.core_properties, field,
            datetime.datetime(2026, 11, 21, 10, 30, 15, tzinfo=datetime.timezone(datetime.timedelta(hours=2))))
    assert stored_text(naive, field) == stored_text(aware, field) == "2026-11-21T08:30:15Z"
    reopened = type(naive).from_bytes(aware.to_bytes())
    read = getattr(reopened.core_properties, field)
    assert read == datetime.datetime(2026, 11, 21, 8, 30, 15, tzinfo=UTC)
    assert read.utcoffset() == datetime.timedelta(0)


@BOTH
@pytest.mark.parametrize("field", FIELDS)
def test_fractional_seconds_are_dropped_when_saved(package, field):
    doc = blank(package)
    setattr(doc.core_properties, field, datetime.datetime(2026, 11, 21, 8, 30, 15, 999999, tzinfo=UTC))
    assert stored_text(doc, field) == "2026-11-21T08:30:15Z"


@BOTH
@pytest.mark.parametrize("field", FIELDS)
def test_a_string_date_is_refused(package, field):
    doc = blank(package)
    with pytest.raises(TypeError, match="datetime"):
        setattr(doc.core_properties, field, "2026-11-21T10:00:00Z")


@pytest.mark.parametrize("package", [
    oxdocx,
    pytest.param(oxpptx, marks=pytest.mark.xfail(strict=True, reason="#303: PowerPoint keeps an empty date element")),
], ids=["docx", "pptx"])
@pytest.mark.parametrize("field", FIELDS)
def test_none_removes_a_date(package, field):
    doc = with_stored_date(package, field, "2026-11-21T10:00:00Z")
    setattr(doc.core_properties, field, None)
    assert getattr(doc.core_properties, field) is None
    assert getattr(type(doc).from_bytes(doc.to_bytes()).core_properties, field) is None
    assert stored_text(doc, field) is None


@BOTH
def test_an_absent_author_reads_as_an_empty_string(package):
    doc = blank(package)
    doc.core_properties.title = "A title"
    doc.core_properties.author = None
    assert doc.core_properties.author == ""
