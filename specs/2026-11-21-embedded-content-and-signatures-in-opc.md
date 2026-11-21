---
date: 2026-11-21
status: draft
scope: pptx
---

# Embedded content and signatures in oxoffice-opc

## Context

Plan item 22 of the [code inspection spec](2026-11-21-code-inspection-and-refactoring.md): embedded content (OLE
objects, ActiveX controls, VBA projects) and the package-signature checks exist twice. They are
`crates/oxoffice-docx/src/embedded.rs` (3,274 lines) and `crates/oxoffice-pptx/src/embedded.rs` (1,817 lines).
PowerPoint's copy is the weaker one (#83).

Measured on main 56b485b, comparing functions by name and text:

- **22 identical functions.**
- **26 that differ:**
  - 11 only by the host (`Document` or `Presentation`), its staging or its error constructors;
  - 12 by a check;
  - 3 because they are format-specific: finding the owner part, and removing an XML reference from a story or a slide.
- **43 functions only in Word and 10 only in PowerPoint.** Most of Word's are generic package checks: the normalised
  pack URI test, the exact content-type test, unrelated signature relationships, owner range collisions, the strict
  XML 1.0 check of ActiveX parts, and the markup compatibility grammar. The rest walk Word stories.
- **Word is the stricter side in every check but one.** PowerPoint compares reachability without case, as OPC part
  names require. Word compares with case, so it can delete a part that is still referenced under another case.
- **Same public API in both facades:** four types (`EmbeddedContentKind`, `EmbeddedSignatureState`,
  `EmbeddedMutationPolicy`, `EmbeddedContentInfo`) and four methods (`embedded_content`, `extract_`, `replace_` and
  `remove_embedded_content`). Nothing else in the workspace (CLI, Python, tests) calls them.
- **The signature graph runs on every PowerPoint save and commit**, through
  `retained_package_signature_would_be_invalidated` (pptx `lib.rs`: `saved_package`, `to_bytes_as`,
  `to_encrypted_bytes`, `commit_candidate`, `publish_authoring_package`). It does not run only on embedded calls.
- **No test covers any of this.** No test calls the four methods or builds a signed, OLE, ActiveX or VBA package, and
  the corpus has no `oleObj` owner.
- **`oxoffice-opc` already has relationship types, part lookup without case and a signature module.** But that module
  compiles only with the `digital-signatures` feature, and its `discover_signature_parts` lacks Word's single-origin,
  misplaced-relationship and content-type checks.

## Decision

- **`oxoffice-opc` owns the package side, in a new module compiled in every build** (not behind
  `digital-signatures`).
  - It holds: the four types; the signature graph and its invalidation markers; VBA and ActiveX resolution; the target
    and identity checks; reachability; and removing relationships and parts.
  - `embedded_info` moves too, which makes `sha2` a required dependency of opc. The strict XML 1.0 check makes opc
    depend on `oxoffice-core`. Both edges keep the dependency rule: shared crates only, and no cycle.
- **Each facade keeps the format-specific part** and implements a small trait: list the owner references in a part,
  remove one, stage a candidate, commit it. Word walks its stories and PowerPoint its slides, layouts and masters, as
  today.
- **The API does not change.**
  - The four types move to opc, and both facades `pub use` them.
  - The four methods stay facade methods.
  - The facade functions that the host files call (`known_invalid_package_signature_on_open`,
    `retained_package_signature_would_be_invalidated`, `persist_invalidated_package_signature`,
    `synchronized_package_mutation_invalidates_signature`, `relationship_target_is_normalized_pack_uri`,
    `must_understand_prefix_is_valid`) keep their names in each `embedded.rs`, as thin wrappers or re-exports.
  - So docx `document.rs` and pptx `lib.rs`, the wave 3 files, are not edited. Neither are `flat_opc.rs`,
    `field/update.rs` and `bibliography/source.rs`.
- **The shared code keeps Word's checks, with one exception:** reachability compares part names without case, as
  PowerPoint does and OPC requires.
- **Errors:** the shared module returns its own error kinds, and each facade maps them to the errors it returns today.
  PowerPoint keeps `MissingRelationship`, `ExternalRelationship`, `MissingPart` and `MalformedPart`. Word keeps
  `InvalidEmbeddedMutation`, and PowerPoint uses `InvalidEmbeddedMutation` for the new refusals.
- **PowerPoint refuses more, so this is `fix(pptx)!` with a `BREAKING CHANGE:` footer.** Closes #83.
  - Embedded content, VBA, origin and signature relationships whose target is not a normalised pack URI: non-ASCII,
    spaces, lowercase or needless `%` escapes, `?`, `#`, `\`, `//`, `.` segments.
  - A `TargetMode` that is neither absent nor exactly `Internal`: it counts as external.
  - In a signed package:
    - an origin or signature part without its exact content type;
    - another relationship to the origin, or to a signature part from outside the origin.
  - A VBA signature part without its exact content type.
  - ActiveX properties without their content type, not strict XML 1.0, or not UTF-8. A control part without exactly
    one binary. More than one VBA project.
  - Removing a VBA project whose target another relationship shares.
  - Unsigned packages save exactly as before: the branch with no origin is the same in both copies.
- **Not done here:** sharing the markup compatibility grammar. It reads Word's story walker state, and PowerPoint does
  not use it today. It moves when a second user needs it.

## Risks

- **A signed `.pptx` that saves today could fail to save, or to commit any edit**, because the stricter graph runs on
  every save. Real PowerPoint output should pass: oxoffice's own `create_signature` writes the exact content types.
  Before step 3, the stricter checks run over every `.pptx` and `.docx` in the corpus and the oxoffice-skills fixtures.
  Any real file they would refuse stops the plan for a decision.
- **Word changes once:** reachability without case keeps a part that Word used to delete when it was still
  referenced under another case. That is a fix; it gets its own commit and a test.
- **No test covers this code today.** The tests come first (step 1) and pin today's behaviour of both facades.

## Plan

Each step is reviewed on the local branch before its one push.

1. `test(docx,pptx): embedded content and package signatures as they behave today`
   - Fixture packages built in the tests, no binary blobs: OLE in a Word story and on a slide, an ActiveX control
     with its binary, a VBA project with its signature, and a package signed with `create_signature`.
   - The four methods, the policy choices, and save after an edit of a signed package.
   - The PowerPoint cases that step 3 will refuse are written as today's answers, and flip in step 3.
2. `refactor(opc): embedded content and signature checks in one module`, then Word on it.
   - The module, the trait, and Word's `embedded.rs` delegating to it. Word's behaviour is unchanged.
   - Then `fix(docx): a part still referenced under another case is not deleted`, with its test.
3. `fix(pptx)!: embedded content and signatures check what Word checks` (closes #83).
   - PowerPoint's `embedded.rs` delegates to opc, and the step 1 tests flip where listed above.
   - It comes after the corpus run.
4. Spec status to `shipped` with the learnings.

None of these steps edits a wave 3 file, so they can run during wave 3. A step that would need to edit one waits for
that file's split.

## Links

- [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md), items 22 and 24.
- [Python core property dates and shared helpers](2026-11-21-python-core-property-dates-and-shared-helpers.md),
  item 23.
- #83: PowerPoint embedded content and signatures skip two checks that Word makes.
