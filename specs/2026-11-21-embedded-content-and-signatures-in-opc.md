---
date: 2026-11-21
status: shipped
scope: opc, docx, pptx
---

# Embedded content and signatures in oxoffice-opc

## Context

Plan item 22 of the [code inspection spec](2026-11-21-code-inspection-and-refactoring.md): embedded content (OLE
objects, ActiveX controls, VBA projects) and the package-signature checks exist twice. They are
`crates/oxoffice-docx/src/embedded.rs` (3,274 lines) and `crates/oxoffice-pptx/src/embedded.rs` (1,817 lines).
PowerPoint's copy is the weaker one (#83).

Measured on main 78f44de, comparing functions by name and text. Neither file changed between the import and the
parent spec, so its "16 diverged" was a miscount. Since then #218 moved Word's three NCName functions to core (40
Word-only functions, 3,239 lines).

- **22 identical functions.**
- **26 that differ:**
  - 11 only by the host (`Document` or `Presentation`), its staging or its error constructors;
  - 12 by a check;
  - 3 because they are format-specific: finding the owner part, and removing an XML reference from a story or a slide.
- **43 functions only in Word and 10 only in PowerPoint.** Most of Word's are generic package checks:
  - the normalised pack URI test and the exact content-type test;
  - unrelated signature relationships and owner range collisions;
  - the strict XML 1.0 check of ActiveX parts;
  - the markup compatibility grammar.

  The rest walk Word stories.
- **Word is the stricter side in every check but one.** PowerPoint compares reachability without case, as OPC part
  names require. Word compares with case.
- **Word's target check reaches far.** `reject_unrelated_signature_incoming` and Word's reachability run the
  normalised pack URI test on every internal relationship of the package, not only on embedded content.
- **"Would a save invalidate the signature?" is answered differently.** PowerPoint compares the written bytes of the
  staged and current packages, writing two whole packages on every call for a signed package. Word compares the package model
  (`packages_are_semantically_equal`).
- **Same public API in both facades:** four types (`EmbeddedContentKind`, `EmbeddedSignatureState`,
  `EmbeddedMutationPolicy`, `EmbeddedContentInfo`) and four methods (`embedded_content`, `extract_`, `replace_` and
  `remove_embedded_content`). Nothing else in the workspace (CLI, Python, tests) calls them.
- **The signature graph runs on every PowerPoint save and commit.** It does not run only on embedded calls.
  - Through `retained_package_signature_would_be_invalidated`: `saved_package` (behind `to_bytes` and `save`),
    `to_bytes_as`, `to_encrypted_bytes`, `commit_owned_parts` and `commit_candidate`.
  - Through `persist_invalidated_package_signature`: `publish_authoring_package`.
- **No test covers any of this.**
  - No test calls the four methods or builds a signed, OLE, ActiveX or VBA package.
  - The 64 corpus files include none that is signed, none with OLE, ActiveX or VBA content, and none with a target
    that is not a normalised pack URI.
- **`oxoffice-opc` already has relationship types, part lookup without case and a signature module.** But that module
  compiles only with the `digital-signatures` feature, and its `discover_signature_parts` lacks Word's single-origin,
  misplaced-relationship and content-type checks.

## Decision

- **`oxoffice-opc` owns the package side, in a new module compiled in every build** (not behind
  `digital-signatures`).
  - It holds: the four types; the signature graph and its invalidation markers; VBA and ActiveX resolution; the target
    and identity checks; owner range collisions; reachability; and removing relationships and parts.
  - `embedded_info` moves too, which makes `sha2` a required dependency of opc. The strict XML 1.0 check makes opc
    depend on `oxoffice-core`. Both edges keep the dependency rule: shared crates only, and no cycle.
- **Each facade keeps the format-specific part** and implements a small trait: list the owner references in a part,
  remove one, stage a candidate, commit it. Word walks its stories and PowerPoint its slides, layouts and masters, as
  today.
- **The API does not change.**
  - The four types move to opc, and both facades `pub use` them.
  - The four methods stay facade methods.
  - The facade functions that host files call keep their names in each `embedded.rs`, as thin wrappers or
    re-exports: `known_invalid_package_signature_on_open`, `retained_package_signature_would_be_invalidated`,
    `persist_invalidated_package_signature`, `synchronized_package_mutation_invalidates_signature`,
    `relationship_target_is_normalized_pack_uri` and `must_understand_prefix_is_valid`.
  - So docx `document.rs` and pptx `lib.rs`, the wave 3 files, are not edited. Neither are `flat_opc.rs`,
    `field/update.rs` and `bibliography/source.rs`.
- **The shared code keeps Word's checks, with one exception:** reachability compares part names without case, as
  PowerPoint does and OPC requires. "Would a save invalidate the signature?" uses Word's model comparison.
- **Errors:** the shared module returns its own error kinds, and each facade maps them to the errors it returns today.
  PowerPoint keeps `MissingRelationship`, `ExternalRelationship`, `MissingPart` and `MalformedPart`. Word keeps
  `InvalidEmbeddedMutation`. PowerPoint uses `InvalidEmbeddedMutation` for the new refusals.
- **PowerPoint refuses more, so this is `fix(pptx)!` with a `BREAKING CHANGE:` footer.** Closes #83.
  - **Save and commit of a signed package fail when any internal relationship of the package has a target that is
    not a normalised pack URI**, an image or a slide jump included. Examples: non-ASCII, a space, lowercase or needless
    `%` escapes, `?`, `#`, `\`, `//`, `.` segments.
  - On a signed package, the save also fails on:
    - an origin or signature part without its exact content type;
    - another relationship to the origin, or to a signature part from outside the origin.
  - Removing embedded content fails, signed or not, when any internal relationship of the package has such a target.
  - Embedded content, VBA, origin and signature relationships need normalised pack URI targets, and a `TargetMode`
    that is neither absent nor exactly `Internal` counts as external.
  - A VBA signature part needs its exact content type.
  - For ActiveX, the properties need their content type, strict XML 1.0 and UTF-8, and a control part needs exactly
    one binary.
  - More than one VBA project is refused.
  - Removing a VBA project whose target another relationship shares is refused.
  - Two owners claiming the same range are refused (owner range collisions).
  - PowerPoint reports a signature invalidated less often: a save that changes the package bytes but not its model no
    longer counts.
  - Unsigned packages save exactly as before: the branch with no origin is the same in both copies.
- **Word changes twice, both from reachability without case:**
  - a part still referenced under another case is no longer deleted;
  - removing a VBA project whose target another relationship names under another case is now refused as shared.
- **Not done here:** sharing the markup compatibility grammar. It reads Word's story walker state, and PowerPoint does
  not use it today. It moves when a second user needs it.

## Risks

- **A signed `.pptx` that saves today could fail to save, or to commit any edit**, because of one unrelated relationship
  with a non-normalised target.
  - No file we hold can measure this: none of the 64 corpus files has such a target, and none is signed.
  - So this refusal rests on the step 1 test, which pins it, and on the footer, which states it. It does not rest on a
    measurement.
  - Before step 3, a run signs every corpus file with the test key, saves it after one edit, and checks it with the
    stricter graph. That run tests the content-type and incoming-relationship checks on real files.
  - A refusal there, or a user report of a real file refused, stops the plan for a decision. That decision could be
    to narrow Word's full-package target check to the signature and embedded relationships, which would relax Word
    too.
- **Word changes**: the two reachability changes above are fixes. They get their own commit and tests.
- **No test covers this code today.** The tests come first (step 1) and pin today's behaviour of both facades.
- **Item 15 (debug work counters)** lists pptx `embedded.rs` and opc `lib.rs` among its files; its first pull
  request, #211, does not touch them yet. Steps 2 and 3 wait until item 15 is done with those two files.

## Plan

Each step is reviewed on the local branch before its one push.

1. `test(docx,pptx): embedded content and package signatures as they behave today`
   - Fixture packages built in the tests, no binary blobs: OLE in a Word story and on a slide, an ActiveX control with
     its binary, and a VBA project with its signature.
   - A package signed with `create_signature`, using a test-only RSA key and self-signed certificate committed as PEM
     next to the tests, marked as such, with the command that made them.
     - Why not generate them in the test: opc signs with RSA (`ring::signature::RsaKeyPair`), and `ring` cannot
       generate RSA keys. Nothing in the dependency tree can (no `rsa`, no `rcgen`).
     - Generating them would add new dev-dependencies with their own trees and licence review, and seconds of key
       generation per run.
     - The key protects nothing.
   - The tests run with `--features digital-signatures`. Adding that to CI is settled with the CI session (OX-C2).
   - They cover the four methods, the policy choices, and a save after an edit of a signed package.
   - Every PowerPoint refusal listed above is written as today's answer, on both facades:
     - on Word it must already refuse, so step 2 is checked by tests rather than assumed;
     - on PowerPoint it flips in step 3.
   - Word's two reachability changes are written as today's answers too; they flip in step 2's fix.
   - This includes the signed deck with an image target containing a raw space.
2. `refactor(opc): embedded content and signature checks in one module`, then Word on it.
   - Adds the module and the trait, and Word's `embedded.rs` delegates to it. Word's behaviour is unchanged: the step
     1 tests pass as they are.
   - Then `fix(docx): reachability ignores case`, with tests for its two changes.
   - `docs/ARCHITECTURE.md`: opc gains the embedded-content and signature checks.
3. **Gate: a brief to the orchestrator before any code of this step is pushed.** It carries:
   - (a) the result of the signed corpus run of the Risks section;
   - (b) an answer, with code, to one question. Why refuse to save a signed package over an unrelated non-normalised
     target (an image), rather than normalise the target and refuse only when it can name a protected part (the
     origin, a signature, embedded content)?
     - #83 shows no demonstrated failure, and a user who can no longer save a deck is a real cost.
     - If the narrower rule holds, it replaces Word's full-package target check on both sides, and the refusal list
       above shrinks to match.
   - The orchestrator takes this breaking change to the user with that answer.
   - Then `fix(pptx)!: embedded content and signatures check what Word checks` (closes #83):
     - PowerPoint's `embedded.rs` delegates to opc;
     - the step 1 tests flip where listed above.
4. Spec status to `shipped` with the learnings.

None of these steps edits a file that wave 3 splits. Steps 2 and 3 wait until item 15 is done with pptx `embedded.rs` and opc
`lib.rs`.

## Amendment 2026-11-24

Taken at the step 3 gate, before step 2 was built; it replaces Word's full-package target check in step 2.

- **Why.** The checks compared part names with raw `==`. Word refused every non-normalised internal target of a signed
  package (and on remove) only to keep `==` honest: `../%5Fxmlsignatures/origin.sigs` names the origin to a decoder but
  not to `==`. It still missed case: `../_XMLSIGNATURES/ORIGIN.SIGS` passes the normalised test, opc's lookup finds the
  origin, `==` does not. The check was neither necessary nor sufficient.
- **The rule, in opc.**
  - One canonical part identity, compiled in every build: resolve the target against its source part, percent-decode,
    collapse dot and empty segments, fold ASCII case. A target holding invalid UTF-8, an encoded `/` or `\`, a control
    character, a climb above the root or a scheme, or an empty one, names no part and is refused. A target with `\`, `?` or `#` is read
    every way (as written, `\` as `/`, without its query or fragment); every reading must resolve.
  - Every comparison of part names in the shared checks uses it: unrelated relationships to the origin or a signature,
    reachability, the shared VBA target, the owners of one ActiveX control, delete-if-unreachable.
  - The relationships that define a protected part (origin, signature, OLE, ActiveX control and binary, VBA project
    and signature, the invalidation markers) still need normalised pack URI targets.
  - Any other target is refused only when it names no part, or names a protected part without being a normalised pack
    URI. An image whose target holds a raw space is accepted.
  - Two stored parts with one identity once decoded are refused as ambiguous by a signed save and an embedded mutation.
    Opening a package refuses nothing new.
- **What it changes for Word.**
  - A signed save and a removal (signed or not) accept an unrelated non-normalised target to an ordinary part.
  - A signed save refuses an unrelated relationship naming the origin under another case.
  - Reachability ignores case and escapes: a part still named under another case is kept, and a VBA project whose
    target another relationship names under another case is refused as shared.
  - Two controls naming one ActiveX part under another case are one control: listed once, and removed with both owners
    (Word refused that removal before).
  - A removal (signed or not) and a signed save refuse a target that was a normalised pack URI but names no part
    once decoded: an escape to invalid UTF-8 (`media/caf%E9.png`) or to a control character (`%C2%85`).
- **Step 3 shrinks.** PowerPoint gains the same rule: its refusal list loses "any internal relationship with a
  non-normalised target" and keeps the normalised targets of the protected relationships. The user approved this
  breaking change on 2026-11-24.
- **Corpus.** 70 Office files, 3,214 internal relationships: none with `%`, none non-normalised, no case-only alias.
  Neither rule refuses a file we hold.

## Outcome

Shipped on 2026-11-24 in two pull requests: #609 (the shared module, Word delegating, canonical part identities
per the amendment) and #611 (PowerPoint delegating, a breaking change). A signed corpus run saved 16 decks and
36 documents with none refused.

## Links

- [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md), items 15, 22 and 24.
- [Python core property dates and shared helpers](2026-11-21-python-core-property-dates-and-shared-helpers.md),
  item 23.
- #83: PowerPoint embedded content and signatures skip two checks that Word makes.
