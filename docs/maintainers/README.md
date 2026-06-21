# AgentZero Documentation Inventory

This inventory classifies docs by intent so readers can quickly distinguish runtime-contract guides from design proposals.

Last reviewed: **June 11, 2026**.

## Classification Legend

- **Current Guide/Reference**: intended to match current runtime behavior
- **Policy/Process**: collaboration or governance rules
- **Proposal/Roadmap**: design exploration; may include hypothetical commands

## Documentation Entry Points

| Doc | Type | Audience |
|---|---|---|
| `README.md` | Current Guide | all readers |
| `docs/README.md` | Current Guide (hub) | all readers |
| `docs/SUMMARY.md` | Current Guide (unified TOC) | all readers |

## Collection Index Docs

| Doc | Type | Audience |
|---|---|---|
| `docs/reference/README.md` | Current Guide (reference hub) | users/operators |
| `docs/ops/README.md` | Current Guide (operations hub) | operators |
| `docs/security/README.md` | Current Guide (security hub) | operators/contributors |
| `docs/hardware/README.md` | Current Guide (hardware hub) | hardware builders |
| `docs/contributing/README.md` | Current Guide (contributing hub) | contributors/reviewers |
| `docs/setup-guides/README.md` | Current Guide (setup hub) | new users |
| `docs/architecture/README.md` | Current Guide (architecture hub) | all readers |
| `docs/maintainers/README.md` | Current Guide (maintainers hub) | maintainers |

## Current Guides & References (API/CLI)

| Doc | Type | Audience |
|---|---|---|
| `docs/reference/cli/commands-reference.md` | Current Reference | users/operators |
| `docs/reference/api/providers-reference.md` | Current Reference | users/operators |
| `docs/reference/api/channels-reference.md` | Current Reference | users/operators |
| `docs/reference/api/config-reference.md` | Current Reference | operators |

## Current Setup & Integration Guides

| Doc | Type | Audience |
|---|---|---|
| `docs/setup-guides/one-click-bootstrap.md` | Current Guide | users/operators |
| `docs/setup-guides/zai-glm-setup.md` | Current Provider Setup Guide | users/operators |
| `docs/setup-guides/windows-setup.md` | Current Guide | Windows users |
| `docs/setup-guides/macos-update-uninstall.md` | Current Guide | macOS users |
| `docs/setup-guides/mcp-setup.md` | Current Guide | MCP integration users |
| `docs/contributing/custom-providers.md` | Current Integration Guide | integration developers |
| `docs/browser-setup.md` | Current Guide | users configuring browser automation |

## Current Operations & Deployment

| Doc | Type | Audience |
|---|---|---|
| `docs/ops/operations-runbook.md` | Current Guide | operators |
| `docs/ops/troubleshooting.md` | Current Guide | users/operators |
| `docs/ops/network-deployment.md` | Current Guide | operators |

## Current Hardware Guides

| Doc | Type | Audience |
|---|---|---|
| `docs/hardware/arduino-uno-q-setup.md` | Current Guide | hardware builders |
| `docs/hardware/nucleo-setup.md` | Current Guide | hardware builders |
| `docs/hardware/android-setup.md` | Current Guide | mobile hardware users |
| `docs/hardware/hardware-peripherals-design.md` | Current Design Spec | hardware contributors |
| `docs/hardware/datasheets/nucleo-f401re.md` | Current Hardware Reference | hardware builders |
| `docs/hardware/datasheets/arduino-uno.md` | Current Hardware Reference | hardware builders |
| `docs/hardware/datasheets/esp32.md` | Current Hardware Reference | hardware builders |

## Current SOP & Integration Guides

| Doc | Type | Audience |
|---|---|---|
| `docs/reference/sop/README.md` | Current Guide | operators |
| `docs/reference/sop/syntax.md` | Current Reference | operators |
| `docs/reference/sop/connectivity.md` | Current Guide | operators |
| `docs/reference/sop/cookbook.md` | Current Guide | operators |
| `docs/reference/sop/observability.md` | Current Guide | operators |
| `docs/consolidation-integration.md` | Current Integration Spec | operators/contributors |

## Policy / Process Docs

| Doc | Type |
|---|---|
| `docs/contributing/pr-workflow.md` | Policy |
| `docs/contributing/reviewer-playbook.md` | Process |
| `docs/contributing/pr-discipline.md` | Policy |
| `docs/contributing/ci-map.md` | Process |
| `docs/contributing/actions-source-policy.md` | Policy |
| `docs/contributing/testing.md` | Process |
| `docs/contributing/release-process.md` | Process |

## Contributing & Reference Docs

| Doc | Type | Audience |
|---|---|---|
| `docs/reference/skills-authoring.md` | Current Guide | skill developers |
| `docs/contributing/adding-boards-and-tools.md` | Current Guide | hardware/tool contributors |
| `docs/contributing/extension-examples.md` | Current Guide | extension developers |
| `docs/contributing/docs-contract.md` | Current Guide | doc contributors |
| `docs/contributing/doc-template.md` | Current Template | doc contributors |
| `docs/contributing/change-playbooks.md` | Current Playbook | contributors |
| `docs/contributing/label-registry.md` | Current Reference | maintainers |
| `docs/contributing/cargo-slicer-speedup.md` | Current Guide | contributors |

## Proposal / Roadmap Docs

These are valuable context, but **not strict runtime contracts**.

| Doc | Type |
|---|---|
| `docs/security/sandboxing.md` | Proposal |
| `docs/ops/resource-limits.md` | Proposal |
| `docs/security/audit-logging.md` | Proposal |
| `docs/security/agnostic-security.md` | Proposal |
| `docs/security/frictionless-security.md` | Proposal |
| `docs/security/security-roadmap.md` | Roadmap |

## Architecture & Design Docs

| Doc | Type | Audience |
|---|---|---|
| `docs/architecture/agent-prompt-flow.md` | Current Design | contributors |
| `docs/architecture/adr-004-tool-shared-state-ownership.md` | Architecture Decision | contributors |
| `docs/assets/architecture-diagrams.md` | Current Reference | all readers |

## Maintainers & Reference Docs

| Doc | Type | Audience |
|---|---|---|
| `docs/maintainers/repo-map.md` | Current Reference | maintainers |
| `docs/maintainers/refactor-candidates.md` | Current Analysis | maintainers |
| `docs/maintainers/i18n-coverage.md` | Current Status | maintainers |
| `docs/PROVIDER_API_SUPPORT.md` | Current Reference | operators/developers |
| `docs/openai-temperature-compatibility.md` | Current Guide | OpenAI users |

## Maintenance Recommendations

1. Update `docs/reference/cli/commands-reference.md` whenever CLI surface changes.
2. Update `docs/reference/api/providers-reference.md` when provider catalog/aliases/env vars change.
3. Update `docs/reference/api/channels-reference.md` when channel support or allowlist semantics change.
4. Update `docs/reference/api/config-reference.md` when configuration schema changes.
5. Keep setup guides current with new platforms/integration methods.
6. Mark proposal docs clearly to avoid being mistaken for runtime contracts.
7. Update `docs/SUMMARY.md` and collection indexes whenever new major docs are added.
8. **Remove docs-inventory entries when files are deleted.** Don't just mark as removed—purge entries to keep the inventory current.
9. Review this inventory quarterly to catch obsolete file references.
