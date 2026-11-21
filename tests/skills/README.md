# Skill examples

The Word and PowerPoint skills of [oxoffice-skills](https://github.com/oxoffice-dev/oxoffice-skills) are
documentation and examples only, with no tests of their own. Their examples are the contract between the two
repositories: every push to `main` runs them here, against the wheel and the `oxoffice` command the CI just built
(the `skill-examples` job of `.github/workflows/ci.yml`). Pull requests do not run them.

| File | What |
| --- | --- |
| `make_fixtures.py` | writes the inputs: `report.docx`, `deck.pptx`, `Aileron-Regular.ttf`, `photo.png` |
| `fixtures/make_fixture_{docx,pptx}.py` | the synthetic report and deck, with the traits of files saved by Word and Google Docs |
| `run_examples.py` | runs every python and bash block of each skill's `references/examples.md`, in order |
| `check_outputs.py` | checks the files the examples wrote |
| `check_names.py` | checks that every function, keyword, attribute, command and flag the skills cite exists |

Locally, in a virtual environment with `.github/requirements/ci.txt` and the wheel installed (a `scaling` build
runs them in about 3 minutes, a `ci` build in about 9):

```bash
git clone https://github.com/oxoffice-dev/oxoffice-skills ../oxoffice-skills
python tests/skills/make_fixtures.py /tmp/skills/fixtures
python tests/skills/run_examples.py --skills ../oxoffice-skills/plugin/skills --fixtures /tmp/skills/fixtures --work /tmp/skills
python tests/skills/check_outputs.py /tmp/skills
python tests/skills/check_names.py ../oxoffice-skills/plugin/skills
```

A failure means one of two things: a change on `main` broke what the skills promise (fix it here, or say so in
the pull request that changes the API, and update the skills), or the skills cite something that does not exist
(fix the skills). Each block's code and output are in `<work>/<skill>/_blockNN.log`, uploaded by the job when it
fails.
