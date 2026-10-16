# Fabushi iOS — Agent Instructions

These instructions apply repository-wide to AI-assisted development in `bhrumom/fabushi-ios`.

## CRITICAL: Repository ownership

This repository is the canonical source for **native iOS application, iOS-specific UI/runtime integration, signing and App Store/TestFlight delivery**.

- Verify the current GitHub repository before product-affecting work.
- Do not implement another Fabushi platform's product code here. Switch to that platform's canonical repository first.
- `bhrumom/fabushi` is the legacy migration/source-history repository, not the canonical implementation repository for this scope.
- Shared Mahayana/Rust runtime/contracts belong in `bhrumom/fabushi-platform-core`; this repository owns only its platform-specific consumer/integration.

## CRITICAL: Spec-first development — No Spec, No Code

Before changing application/runtime code, tests, schemas, contracts, dependencies, build/release configuration, migrations, security controls, or other behavior-affecting files:

1. Read this `AGENTS.md`.
2. Find and read the applicable durable Spec/project/source-of-truth documents.
3. Check `docs/specs/` for a task/feature Spec.
4. Validate the Spec against the latest explicit user requirement and current repository/GitHub facts.
5. If no usable Spec exists, or it is stale/unclear/contradictory, create or repair the Spec **before implementation** using `docs/specs/SPEC_TEMPLATE.md`.

Read-only investigation needed to understand the system or write the Spec is allowed first. Product-affecting implementation is not.

## Mandatory lifecycle

**Discover → Spec → Architecture/Plan → Implement → Verify → Spec Compliance Review → Integrate/Deliver**

Before completion, compare the implementation against every applicable requirement and acceptance criterion and record `passed`, `blocked`, or `not-applicable` with evidence/reason.

## Fail-closed rules

Do not start product-affecting implementation without a usable Spec; do not use chat memory as the only durable requirement source; do not silently change scope or weaken acceptance criteria; update the Spec when design/behavior changes intentionally.

Canonical policy: `docs/specs/spec-first-ai-development.md`.
