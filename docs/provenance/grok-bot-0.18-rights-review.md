# Grok Bot 0.18 rights and provenance review

Status: **release-blocking review open**

This record separates technical parity evidence from rights/provenance
clearance. A successful build, test, archive, or App Store Connect upload does
not by itself resolve redistribution rights.

## Pinned technical reference

The architecture/parity reference is:

`b-nnett/grok-bot-0.18-reconstructed@a9f633e09d49a85829b8236331b9e21f7e612634`

That reference identifies itself as an unofficial reconstruction of the
publicly distributed Grok Bot 0.18.0 application. Its NOTICE/PROVENANCE records
state, in substance, that no upstream source-code license is granted for the
reconstructed implementation and that independent copyright, trademark,
third-party dependency, and service-terms review is required before public
redistribution.

The same reference records the public binary provenance it studied, including
Grok Bot 0.18.0 macOS/Windows release artifacts. Those binary artifacts are
research evidence only and are not release inputs for this iOS repository.

## Fabushi source provenance

The standalone repository export records:

- `bhrumom/fabushi@7851b689d2fe3fc3893cd9f4363899cc4a03e83b`
- target boundary `ios`
- exported roots `mobile/ios;mobile/native/include`

Subsequent parity work is tracked file-by-file in
`docs/parity/grok-bot-0.18-ios-parity-ledger.csv`. The ledger records the
pinned reference path, the iOS target/adaptation, implementation status, test
evidence, and adaptation rationale. It is not a license manifest.

## Release review checklist

The following items must be resolved before AC-23 may be marked `passed`:

- [ ] Independent reviewer confirms which files are original Fabushi work,
      clean-room/evidence-driven reimplementations, third-party licensed code,
      or reconstructed/reference-derived material.
- [ ] Any material that cannot be redistributed under an identified right or
      license is replaced, removed, or separately authorized.
- [ ] Required third-party license texts/notices for shipped source and binary
      dependencies are present.
- [ ] Product names, icons, strings, and other marks are reviewed for trademark
      or attribution obligations.
- [ ] No pinned upstream installers, extracted application payloads, upstream
      signatures, private credentials, or forensic workspaces are included in
      source archives, Xcode archives, IPAs, xcresults, or GitHub release
      artifacts.
- [ ] The final exact release SHA and generated IPA are re-audited after the
      parity ledger reaches its final state.

## Current disposition

The technical work may continue, including CI, unsigned device archives, local
tests, and preparation of a fail-closed App Store/TestFlight delivery workflow.
However, this record does **not** declare reconstructed material safe to
redistribute. Until the checklist above is independently reviewed and recorded,
Spec requirement R16 / acceptance criterion AC-23 remain **blocked**, not
`passed`.
