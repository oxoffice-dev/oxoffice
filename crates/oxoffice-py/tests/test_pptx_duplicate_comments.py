"""slides.duplicate() copies a slide's modern comments, with their replies, under fresh ids, and the copy's
comments can be edited without touching the original's."""

from pathlib import Path

import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
ADA = "{11111111-1111-1111-1111-111111111111}"
THREAD = "{00000000-0000-4000-8000-000000000001}"
REPLY = "{00000000-0000-4000-8000-000000000002}"


def ids(slide):
    return [i for comment in slide.comments for i in (comment.id, *(reply.id for reply in comment.replies))]


def test_a_duplicated_slide_carries_its_comments_under_fresh_ids(tmp_path):
    prs = oxpptx.Presentation(CORPUS / "powerpoint" / "deck.pptx")
    prs.add_comment_author(id=ADA, name="Ada Lovelace", user_id="ada@example.com", provider_id="None",
                           initials="AL")
    slide = prs.slides[1]
    slide.add_comment(id=THREAD, author_id=ADA, created="2026-11-21T10:00:00Z", text="Check the figures")
    slide.reply_to_comment(THREAD, id=REPLY, author_id=ADA, created="2026-11-21T10:05:00Z", text="Done")

    copy = prs.slides.duplicate(slide)

    assert [comment.text for comment in copy.comments] == ["Check the figures"]
    assert [reply.text for reply in copy.comments[0].replies] == ["Done"]
    assert copy.comments[0].author_id == ADA
    assert not set(ids(copy)) & {THREAD, REPLY}
    copy.resolve_comment(copy.comments[0].id)

    path = tmp_path / "copied.pptx"
    prs.save(path)
    reopened = oxpptx.Presentation(path)
    original, copied = reopened.slides[1], reopened.slides[2]
    assert ids(original) == [THREAD, REPLY]
    assert [comment.text for comment in copied.comments] == ["Check the figures"]
    assert copied.comments[0].status == "resolved"
    assert original.comments[0].status is None
    assert len(reopened.comment_authors) == 1


def test_validate_names_a_notes_slide_two_slides_share(tmp_path):
    import re
    import zipfile

    prs = oxpptx.Presentation(CORPUS / "powerpoint" / "deck.pptx")
    prs.slides[0].notes_text = "Shared notes"
    path = tmp_path / "notes.pptx"
    prs.save(path)

    # Relate the third slide, which has no notes of its own, to the first slide's notes.
    shared = tmp_path / "shared.pptx"
    with zipfile.ZipFile(path) as source, zipfile.ZipFile(shared, "w") as target:
        first = source.read("ppt/slides/_rels/slide1.xml.rels").decode()
        notes = re.search(r'Type="[^"]*/notesSlide" Target="([^"]+)"|Target="([^"]+)" Type="[^"]*/notesSlide"', first)
        notes_target = notes.group(1) or notes.group(2)
        for item in source.infolist():
            data = source.read(item.filename)
            if item.filename == "ppt/slides/_rels/slide3.xml.rels":
                data = data.decode().replace(
                    "</Relationships>",
                    '<Relationship Id="rIdShared" Type="http://schemas.openxmlformats.org/officeDocument/2006/'
                    f'relationships/notesSlide" Target="{notes_target}"/></Relationships>',
                ).encode()
            target.writestr(item, data)

    issues = oxpptx.Presentation(shared).validate()
    assert [issue.kind for issue in issues if issue.kind == "shared_slide_part"] == ["shared_slide_part"]
    assert "slides 1, 3" in next(issue.message for issue in issues if issue.kind == "shared_slide_part")
