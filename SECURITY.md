# Security Policy

rustainer is a container isolation tool. Security issues in this project are
by definition high-severity, and we treat them that way.

## Supported versions

The project is pre-release (R&D phase). Only the latest commit on `main` is
supported.

## Reporting a vulnerability

**Do not open a public issue for security problems.**

Email mansurbek1203@gmail.com with:

- A description of the issue and its suspected impact
- Steps to reproduce or a proof of concept
- Affected component (kernel isolation / networking / image handling / cgroups)
- Any known mitigations

You can expect an initial response within 7 days. We will credit reporters in
the fix announcement unless you prefer to remain anonymous.

## Scope

In scope:

- Container escape vectors (namespace, mount, `pivot_root`, procfs/sysfs leaks)
- Privilege boundary violations (rootless mode, user namespaces, capabilities)
- Image handling: layer extraction path traversal, symlink escape, tar CVEs
- Networking isolation failures between containers and host

Out of scope:

- Issues requiring a malicious host kernel
- Social engineering of users into running untrusted images as root

## Research references

This repository contains security *research* describing how container internals
work. Nothing in `research/` is exploit code; claims are tagged and cited.
