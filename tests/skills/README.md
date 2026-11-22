# Skill examples

The Word and PowerPoint skills of [oxoffice-skills](https://github.com/oxoffice-dev/oxoffice-skills) are
documentation and examples only, with no tests of their own yet. Their examples are the contract between the two
repositories. They leave this repository for a CI of their own in oxoffice-skills, against the released package
installed the ways the skills document (user decision, 2026-11-22); until then they run here by hand, below. No job
of `.github/workflows/` runs them any more.

| File | What |
| --- | --- |
| `make_fixtures.py` | writes the inputs: `report.docx`, `deck.pptx`, `Aileron-Regular.ttf`, `photo.png` |
| `fixtures/make_fixture_{docx,pptx}.py` | the synthetic report and deck, with the traits of files saved by Word and Google Docs |
| `run_examples.py` | runs every python and bash block of each skill's `references/examples.md`, in order, or `--jobs N` at a time |
| `check_outputs.py` | checks the files the examples wrote |
| `check_names.py` | checks that every function, keyword, attribute, command and flag the skills cite exists |

Locally, in a virtual environment with `.github/requirements/ci.txt` and the wheel installed (a `scaling` build
runs them in about 1 minute with `--jobs 4`, 2 in order; a `ci` build in about 9):

```bash
git clone https://github.com/oxoffice-dev/oxoffice-skills ../oxoffice-skills
python tests/skills/make_fixtures.py /tmp/skills/fixtures
python tests/skills/run_examples.py --jobs 4 --skills ../oxoffice-skills/plugin/skills --fixtures /tmp/skills/fixtures --work /tmp/skills
python tests/skills/check_outputs.py /tmp/skills
python tests/skills/check_names.py ../oxoffice-skills/plugin/skills
```

A failure means one of two things: a change here broke what the skills promise (fix it, or say so in the pull
request that changes the API, and update the skills), or the skills cite something that does not exist
(fix the skills). Each block's code and output are in `<work>/<skill>/_blockNN.log`. With `--jobs`, a failure runs
the skill again in order in a fresh folder, and only that run counts.

`check_outputs.py` knows what each example writes: an example whose output changes in the skills changes it too.

`check_names.py` matches a method name against every class of both packages, and an attribute against every word
of their sources: it catches a name that no longer exists anywhere, not one moved to another class.
