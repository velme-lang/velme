# Security policy

Velme runs code that an LLM proposes, so the sandbox and the IR validator are security boundaries
(`docs/spec/tooling/41-security-privacy.md`, INV-1, INV-4).

## Reporting a vulnerability

Please do **not** open a public issue. Use GitHub's private vulnerability reporting on this repository
(Security → Report a vulnerability). Include the Velme version (`velme --version`), a minimal program or IR that
reproduces the problem, and what you expected.

## Supported versions

Velme is pre-release (specification and early implementation). Only `main` is supported until the first tagged
release.
