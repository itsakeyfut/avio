# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| 0.15.x | Yes |
| < 0.15 | No |

While the project is pre-1.0, only the latest `0.x` minor receives security fixes.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

If you discover a security vulnerability, report it privately via
[GitHub Security Advisories](https://github.com/itsakeyfut/avio/security/advisories/new).

Please include:

- A description of the vulnerability and its potential impact
- Steps to reproduce or a minimal proof-of-concept
- Affected crate(s) and version(s)
- Any suggested mitigations if known

You can expect an acknowledgment within **7 days** and a resolution or status update
within **30 days**.

## Scope

This project wraps FFmpeg's libav* libraries. Vulnerabilities in FFmpeg itself should
be reported to the [FFmpeg security team](https://ffmpeg.org/security.html).
Vulnerabilities in the Rust bindings or safe wrapper code belong here.
