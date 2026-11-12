# Verifying a release

How to check that the server image and the npm packages you install are the ones Cavea released, built from its public source by its release workflow, and what each contains. Every release is signed without a long-lived key: the signature names the workflow run that built it, and is recorded in Sigstore's public transparency log.

## The server image

The image `{{image}}` is one index for `linux/amd64` and `linux/arm64`. The index and each architecture's image are signed by the release workflow of `{{repository}}`, for the version's tag.

You need [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) 3.0 or newer, and `jq` for the SBOMs.

1. Verify the signature:

   ```sh
   cosign verify {{image}} \
     --certificate-identity {{repository}}/.github/workflows/release.yml@refs/tags/v{{version}} \
     --certificate-oidc-issuer https://token.actions.githubusercontent.com
   ```

   You should see `The cosign claims were validated` and `The code-signing certificate was verified using trusted certificate authority certificates`, then the signed digest. Any other identity, or none, fails.

2. Pin what you run by that digest rather than by tag, so the image cannot change under you: `{{image}}@sha256:…`.

## Its software bill of materials

Each release carries signed SBOMs in [SPDX](https://spdx.dev) 2.3 JSON. The index carries the source's: every Rust crate and JavaScript package in the lockfiles it was built from. Each architecture's image carries its own: the packages of the operating-system base it is built on (Debian's C library, certificates, time zones) and the server, whose Rust crates the source's SBOM lists. To read the source's:

```sh
cosign verify-attestation {{image}} --type spdxjson \
  --certificate-identity {{repository}}/.github/workflows/release.yml@refs/tags/v{{version}} \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  | jq '.payload | @base64d | fromjson | .predicate | {name, packages: (.packages | length)}'
```

It prints the SBOM's name and how many packages it lists. For an architecture's own, run the same command with `{{image}}` replaced by that image's digest, which `docker buildx imagetools inspect {{image}}` lists for each platform (`{{image}}@sha256:…`).

The same SBOM files are attached to the version's release on GitHub (`{{repository}}/releases/tag/v{{version}}`), with the npm packages' tarballs and `SHA256SUMS`, which lists the checksum of each: download them into one directory and run `sha256sum -c SHA256SUMS` there.

## The npm packages

`{{renderer_package}}`, its React component and `{{server_sdk_package}}` are published from the release workflow with npm provenance: a signed statement of the repository, commit and workflow that built each package. In a project that installs them:

```sh
npm audit signatures
```

You should see that the packages have verified registry signatures and verified attestations. On the package's page at npmjs.com, **Provenance** links to the commit and the workflow run.

## The seat map's script-tag build

The script-tag build (`{{renderer_script}}`) is part of the npm package, so its provenance is the package's. It loads its WebAssembly and glyph atlas with their SHA-384 pinned inside it, so the script you load checks the files it fetches. To pin the script itself, give its tag an `integrity` attribute with the script's own SHA-384 (`openssl dgst -sha384 -binary cavea.min.js | openssl base64 -A`).

> **Note** A release built before the first published one is not signed: if you build the image or the seat map from source, you verify the source you built from.
