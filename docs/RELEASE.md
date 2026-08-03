# Releasing dimnav

A release is one tag push. `.github/workflows/release.yml` does the rest: it builds an
Apple Silicon bundle, signs it with the Developer ID certificate, sends it to Apple for
notarization, staples the ticket, signs the updater artifacts, and opens a **draft**
release with everything attached. You review the draft and publish it by hand.

The draft is deliberate. A stray tag can therefore never put a broken build in front of
anyone — nothing is downloadable, and the updater feed stays on the previous version,
until someone clicks publish.

## The secrets

Nine repository secrets, all consumed verbatim by `release.yml`. `GITHUB_TOKEN` is
injected by Actions; do not create it.

| Secret | Holds | Where it comes from |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | base64 of the `.p12` carrying the Developer ID Application certificate **and its private key** | Keychain Access → My Certificates → export the row that expands to show a key |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password | chosen at export time |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <name> (<TEAMID>)` | `security find-identity -v -p codesigning` |
| `KEYCHAIN_PASSWORD` | any strong random string | `openssl rand -base64 24` |
| `APPLE_API_ISSUER` | App Store Connect issuer UUID | App Store Connect → Users and Access → Integrations |
| `APPLE_API_KEY` | the 10-character key ID — the identifier, not the file | same page |
| `APPLE_API_KEY_BASE64` | base64 of `AuthKey_<KEYID>.p8` | downloaded once, at key creation |
| `TAURI_SIGNING_PRIVATE_KEY` | contents of the minisign private key | `~/.tauri/dimnav.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its passphrase, or empty | chosen at key generation |

Set them without the values ever reaching a terminal echo or shell history:

```bash
R=pichugin/dimnav
base64 -i dimnav.p12 | gh secret set APPLE_CERTIFICATE -R $R
gh secret set APPLE_CERTIFICATE_PASSWORD -R $R        # hidden prompt
gh secret set TAURI_SIGNING_PRIVATE_KEY -R $R < ~/.tauri/dimnav.key
```

Include the team ID in `APPLE_SIGNING_IDENTITY`. The bare common name is ambiguous the
moment the job's temporary keychain holds more than one certificate, and `codesign` picks
arbitrarily rather than failing.

`KEYCHAIN_PASSWORD` protects only the throwaway keychain the job creates and destroys.
Nobody needs to know it; generate it and forget it.

### The updater key is unrecoverable

`~/.tauri/dimnav.key` is paired with the `pubkey` baked into `src-tauri/tauri.conf.json`.
Every installed copy of dimnav verifies updates against that public key, so the private
key can never be rotated in isolation — a release signed by a different key is silently
rejected by every client already in the field, and the only recovery is asking users to
download the new version manually.

Back it up somewhere that survives losing this machine. `.gitignore` blocks `*.key`, so
it will never ride along in a commit; the flip side is that nothing in the repo can
reconstruct it.

To check the passphrase without guessing at the key file:

```bash
printf probe > /tmp/probe.txt
npx tauri signer sign -f ~/.tauri/dimnav.key --password "" /tmp/probe.txt
```

Pass `--password ""` explicitly — omitting it opens an interactive prompt that will hang a
script. Success means the passphrase is empty. Note that the key's KDF field reads `Sc`
either way: rsign2 runs scrypt over an empty password too, so that byte tells you nothing.

## Cutting a release

```bash
npm run bump 0.2.0          # Cargo.toml, package.json, package-lock.json
cargo check --workspace     # refresh Cargo.lock so the commit is self-consistent
```

The workspace `Cargo.toml` is the single source of truth for the version. `src-tauri`
inherits it, and `tauri.conf.json` deliberately omits `version` so the bundler falls back
to the crate version. `scripts/bump-version.mjs` edits the manifests and prints the git
commands, but never commits or tags on your behalf.

Then, by hand:

1. `CHANGELOG.md` — move the `[Unreleased]` entries under `## [0.2.0] - YYYY-MM-DD` and
   add the matching link definition. The release body links here, so it must not still say
   "Unreleased".
2. `docs/FEATURES.md` — anything shipping in this release gets ticked.

```bash
cargo test --workspace && npm run check    # what CI will run anyway
git commit -am "release: v0.2.0"
git push origin main
git tag -a v0.2.0 -m "dimnav 0.2.0" && git push origin v0.2.0
gh run watch
```

Tag with `-a`. GitHub Releases treat an annotated and a lightweight tag identically, but
only an annotated tag is a real object carrying a tagger and a date, and only those are
found by `git describe` or ordered by `--sort=taggerdate`. A lightweight tag cannot be
upgraded in place once pushed.

Only plain `MAJOR.MINOR.PATCH` is accepted, enforced at `scripts/bump-version.mjs:29`.
On macOS the version becomes `CFBundleShortVersionString`, where a malformed value is
rejected at *notarization* — twenty minutes in, long after the build succeeded.

Budget 15–35 minutes: a cold Rust cache plus Apple's notarization queue, which usually
answers in 2–5 minutes but has been known to take far longer. The job allows 90.

### Rehearsing without a tag

`workflow_dispatch` runs the whole pipeline against a branch, which is the cheap way to
prove new signing secrets work:

```bash
gh workflow run Release --ref main
```

It produces a draft named `dimnav main`, because `github.ref_name` is the branch. Draft
releases do not create git tags, so nothing is polluted — but delete it when you are done,
and never publish it:

```bash
gh release delete main --yes
```

## Verifying the draft

Four assets, every time:

```
dimnav_<version>_aarch64.dmg
dimnav_aarch64.app.tar.gz
dimnav_aarch64.app.tar.gz.sig
latest.json
```

The updater artifacts carry the target triple in the name even though the bundler writes
the inner archive as plain `dimnav.app.tar.gz` — check the asset names on the release,
not the `file:` field inside the signature.

A missing `.sig` or `latest.json` means the updater key never reached the bundler. Do not
publish — clients would see no update at all, or worse, a `latest.json` pointing at an
archive they cannot verify.

Then check Gatekeeper against the actual downloaded artifact, not the build directory.
This is what catches a bundle that is correctly signed but never stapled, which fails only
on machines that cannot reach Apple's servers:

`bundle.licenseFile` puts a click-through agreement on the DMG, so a bare `hdiutil attach`
prints the licence and exits with `attach canceled` — it is waiting on stdin. Feed it one
`Y`. Not `yes |`: that floods the pipe and the attach fails just as silently.

```bash
gh release download v0.2.0 -p '*.dmg'
echo Y | hdiutil attach dimnav_0.2.0_aarch64.dmg
spctl -a -vvv -t install /Volumes/dimnav/dimnav.app   # accepted, source=Notarized Developer ID
codesign -dv --verbose=4 /Volumes/dimnav/dimnav.app   # Authority=Developer ID Application …; flags=0x10000(runtime)
xcrun stapler validate /Volumes/dimnav/dimnav.app     # The validate action worked!
hdiutil detach /Volumes/dimnav
```

Finally drag it to `/Applications` on a machine that did not build it and launch it once.
That is the only check that exercises the quarantine attribute a real downloader gets.

## Publishing

```bash
gh release edit v0.2.0 --draft=false --latest
```

Nothing works until this runs. GitHub's `/releases/latest` API and the
`/releases/latest/download/<name>` shortcut both ignore drafts, and both are load-bearing:
the updater endpoint in `tauri.conf.json` is the `download` form, and `site/index.html`
resolves the download button through the API form at page load.

The site needs no redeploy — `pages.yml` only fires on `site/**`, and the button is
resolved client-side from the release feed.

## When it goes wrong

**`A public key has been found, but no private key`** — `TAURI_SIGNING_PRIVATE_KEY` is
missing. `bundle.createUpdaterArtifacts` is `true` and `tauri.conf.json` carries a
`pubkey`, so this is a hard failure, not a warning. The comment in `release.yml` calling
the key "harmless while empty" predates the updater being switched on.

**`::warning::No notarization key configured`** — `APPLE_API_KEY_BASE64` did not land. The
build then *succeeds* and ships un-notarized, which users discover as a Gatekeeper block.
Treat this warning as a failure.

**`error: The specified item could not be found in the keychain`** — the `.p12` was
exported without its private key. Export the expandable row under *My Certificates*, not
the certificate on its own.

**Notarization rejected for an invalid version** — a non-semver version reached
`CFBundleShortVersionString`. Fix the version, retag.

**`Error: The operation was canceled` with `Notarizing …` as the last line** — nothing
failed. GitHub killed the runner at `timeout-minutes`, and the job spent its whole life
blocked on Apple. The build, the signing, and the upload all succeeded; only the verdict
never came.

The submission outlives the runner, so ask Apple directly rather than re-running blind:

```bash
xcrun notarytool history --key <p8> --key-id <id> --issuer <uuid>
xcrun notarytool info <submission-id> --key <p8> --key-id <id> --issuer <uuid>
```

`In Progress` for hours means the upload was held for in-depth analysis. Apple does this
to submissions it does not recognise — which, unavoidably, includes an account's first
ever notarization. **Do not re-run while a submission is held: subsequent submissions from
the same team queue behind it**, so each retry costs another full build and adds another
stuck entry. Wait for the verdict, then re-run. Apple's stated behaviour is that the
service learns to recognise an app, so later releases clear in the usual 2–5 minutes.

Apple's system status page reports the Notary Service healthy during these holds — it
tracks outages, not queue latency. It is not a useful signal here.

**Re-running a failed tag** — delete the draft release and the tag, then push it again:

```bash
gh release delete v0.2.0 --yes
git push --delete origin v0.2.0 && git tag -d v0.2.0
```

## Renewals

Neither of these is a code change; both mean redoing the relevant row in the secrets table.

- The Developer ID Application certificate expires five years after issue. A release
  signed with an expired certificate still validates if it was notarized while the
  certificate was valid — but you cannot sign anything new.
- The App Store Connect API key can be revoked from the same page that created it, and the
  `.p8` is downloadable exactly once.
