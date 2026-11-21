"""A comment or a reply created without a date is dated now, UTC to the second; date= writes that timestamp and
undated=True writes none, on the Word and Google Docs files of the corpus."""

from datetime import datetime, timezone
from pathlib import Path

import pytest

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
PRODUCERS = ["word", "google"]
STAMP = "2026-01-02T03:04:05Z"


def report(producer):
    return oxdocx.Document.from_bytes((CORPUS / producer / "report.docx").read_bytes())


def summary_run():
    """The first run of the summary paragraph, the second body item."""
    return oxdocx.RunRange(start=oxdocx.RunPosition(body_index=1, run_index=0),
                           end=oxdocx.RunPosition(body_index=1, run_index=1))


def date_of(doc, comment_id):
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    return next(c for c in saved.comments if c.id == comment_id).date


def now():
    return datetime.now(timezone.utc).replace(microsecond=0)


@pytest.mark.parametrize("producer", PRODUCERS)
def test_comments_and_replies_without_a_date_are_dated_now(producer):
    doc = report(producer)
    before = now()
    ranged = doc.add_comment(summary_run(), author="Reviewer", text="On a range")
    on_text = doc.add_comment_on_text("Summary", author="Reviewer", text="On a text")
    reply = doc.reply_to(ranged, author="Author", text="A reply")
    after = now()
    for comment_id in (ranged, on_text, reply):
        date = date_of(doc, comment_id)
        assert len(date) == 20 and date.endswith("Z"), date
        # fromisoformat takes a trailing Z from Python 3.11 only.
        assert before <= datetime.fromisoformat(date.replace("Z", "+00:00")) <= after


@pytest.mark.parametrize("producer", PRODUCERS)
def test_date_writes_the_timestamp_and_undated_writes_none(producer):
    doc = report(producer)
    dated = doc.add_comment_on_text("Summary", author="R", text="c", date=STAMP)
    undated = doc.add_comment(summary_run(), author="R", text="c", undated=True)
    reply = doc.reply_to(undated, author="A", text="r", undated=True)
    assert date_of(doc, dated) == STAMP
    assert date_of(doc, undated) is None
    assert date_of(doc, reply) is None


def test_date_and_undated_together_are_refused():
    doc = report("word")
    with pytest.raises(ValueError, match="undated"):
        doc.add_comment_on_text("Summary", author="R", text="c", date=STAMP, undated=True)
    with pytest.raises(ValueError, match="undated"):
        doc.reply_to(doc.comments[0].id, author="A", text="r", date=STAMP, undated=True)


@pytest.mark.parametrize("producer", PRODUCERS)
def test_a_comment_in_a_header_is_dated_like_one_in_the_body(producer):
    doc = report(producer)
    header = next(item for item in doc.story_items if item.story.kind == "header" and item.kind == "paragraph")
    header_run = oxdocx.StoryRunRange(start=oxdocx.StoryRunPosition(item=header, run_index=0),
                                      end=oxdocx.StoryRunPosition(item=header, run_index=1))
    before = now()
    dated_now = doc.add_comment(header_run, author="Reviewer", text="In the header")
    after = now()
    date = date_of(doc, dated_now)
    assert before <= datetime.fromisoformat(date.replace("Z", "+00:00")) <= after
    header = next(item for item in doc.story_items if item.story.kind == "header" and item.kind == "paragraph")
    header_run = oxdocx.StoryRunRange(start=oxdocx.StoryRunPosition(item=header, run_index=0),
                                      end=oxdocx.StoryRunPosition(item=header, run_index=1))
    undated = doc.add_comment(header_run, author="Reviewer", text="c", undated=True)
    assert date_of(doc, undated) is None


@pytest.mark.parametrize("producer", PRODUCERS)
def test_tables_only_anchors_on_a_match_in_a_table_cell(producer):
    doc = report(producer)
    anywhere = doc.add_comment_on_text("Churn", author="R", text="c", date=STAMP)
    in_cell = doc.add_comment_on_text("Churn", author="R", text="c", date=STAMP, tables_only=True)
    by_id = {comment.id: comment for comment in doc.comments}
    assert by_id[anywhere].anchor.start.item.story.kind == "body"
    assert by_id[in_cell].anchor.start.item.story.kind == "table_cell"
    assert by_id[in_cell].anchor_text == "Churn"
    with pytest.raises(oxdocx.DocxError, match="table cells"):
        doc.add_comment_on_text("Churn", author="R", text="c", occurrence=1, tables_only=True)
