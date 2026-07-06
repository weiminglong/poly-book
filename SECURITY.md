# Security Policy

## Supported Versions

Security fixes are applied to the `main` branch.

## Reporting a Vulnerability

Do not open a public GitHub issue for suspected security problems.

Report vulnerabilities to <longweiming665@gmail.com> with:

- A concise description of the issue
- Affected components or file paths
- Reproduction steps or a proof of concept
- Any suggested mitigation if you have one

I will acknowledge receipt as soon as practical, investigate, and coordinate a
fix before public disclosure when possible.

## Scope

Please report issues related to:

- Remote code execution
- Credential or secret exposure
- Data corruption or unauthorized data access
- Dependency vulnerabilities with a practical exploit path in this repository
- Unsafe defaults in deployment or CI configuration

Operational outages, stale market data, or incorrect trading logic that do not
have a security impact should go through the normal issue tracker.

## Unsafe-code policy

`unsafe` is forbidden across the workspace (`[workspace.lints.rust]
unsafe_code = "forbid"`). The single exception is `pb-types`, which overrides
to `deny` for four `#[allow]`-scoped `from_utf8_unchecked` sites in its
fixed-point serializers — each carries a SAFETY comment (the writer emits only
ASCII digits and `.`) and the crate runs under miri in CI. Ten of eleven
crates are provably `unsafe`-free at compile time.
