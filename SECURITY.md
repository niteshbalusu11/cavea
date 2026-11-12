# Security policy

## Reporting a vulnerability

Email **niteshbalusu@gmail.com**. Please do not open a public issue or pull request for a security problem.

A useful report says:

- what is affected (the server, the console, the designer, the seat map, an SDK) and its version or commit;
- what an attacker can do;
- the steps, a request or a small script that reproduces it;
- anything you know about how to fix or avoid it.

You will get a reply acknowledging the report, and updates as it is investigated and fixed.

## Disclosure

Please give us the chance to fix a problem before it is made public. Once a fix is released we publish a security advisory with it and credit the reporter, unless they prefer otherwise. Customers on a support plan are notified directly.

## Fix targets

| Severity | Target |
|---|---|
| Critical | a fix within 72 hours |
| High | a fix within 14 days |

Lower severities are fixed in a normal release.

## Supported versions

Security fixes are made for the latest two minor releases.

## Scope

In scope: everything in this repository as shipped: the server and its image, the console and designer, the seat map and its packages, the Node server SDK.

Out of scope: problems that need a compromised machine or account you already control, denial of service by sheer request volume, and findings from automated scanners with no demonstrated impact.

## Verifying a release

Images are signed and carry a software bill of materials; packages are published with provenance. The documentation's "Verifying a release" page has the commands.
