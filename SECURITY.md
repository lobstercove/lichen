# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Lichen, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

### Contact

- **Email**: security@lichen.network
- **Subject line**: `[SECURITY] <brief description>`

### What to include

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

### Response timeline

- **Acknowledgment**: Within 48 hours
- **Initial assessment**: Within 7 days
- **Fix timeline**: Depends on severity; critical issues are patched within 72 hours

### Scope

The following are in scope:
- Core blockchain (`core/`, `validator/`, `rpc/`, `p2p/`)
- Smart contracts (`contracts/`)
- Custody service (`custody/`)
- SDKs (`sdk/`)
- CLI (`cli/`)

The following are out of scope:
- Frontend-only issues with no backend impact (e.g., CSS rendering)
- Denial of service via high transaction volume (rate limiting is in place)
- Issues in third-party dependencies (report upstream; notify us if it affects Lichen)

### Recognition

We appreciate responsible disclosure. Security researchers who report valid vulnerabilities
will be credited in the fix release notes (unless they prefer to remain anonymous).

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.4.x   | Yes       |
| < 0.4   | No        |
