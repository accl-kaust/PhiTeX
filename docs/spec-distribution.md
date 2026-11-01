# Distribution: Shelf hosting, updates, news, compatibility

Status: design, 2026-10-30. The user hosts Shelf before dev goes to main.
People will run different PhiTeX versions at once, against one server, with
one `phitex.toml` shared in a project's git. Everything here follows from
that.

## 1. Rules

1. **Published files never change.** A URL that has served bytes serves the
   same bytes forever (packs, indexes, release manifests, binaries). New
   things get new URLs. Every such file is cacheable forever.
2. **Old clients keep working.** A release a shipped PhiTeX pins stays
   served. A breaking protocol change gets a new URL prefix, served beside
   the old one, never instead of it.
3. **Readers ignore what they don't know.** Every JSON and TOML reader skips
   unknown fields (with a warning for the user's own files, §5), so a newer
   writer never breaks an older reader. A new field never changes what the
   old fields mean.
4. **Nothing leaves the machine unless the user asked.** Fetching packs
   sends package names (already said in the viewer). The update check is
   off until the user turns it on, and it sends one GET with no identifiers.

## 2. The server: Shelf as it is hosted

Shelf is already hosted on Cloudflare Pages' free plan, from the
shelf.PhiTeX.org repo (`deploy.sh`, `release.py`; its README has the
layout, retention and the measured growth). It already follows §1: packs
are named by their content, every release's index stays, CORS is open, and
a schema an older client can't read gets a Pages project of its own beside
the old one. So the layout stays as it is:

```
<root>/shelf.json                         new: the TeX Live prefixes and each one's latest release (short cache)
<root>/tl2026/release.json                the prefix's latest release (short cache)
<root>/tl2026/release-<release>.json      immutable
<root>/tl2026/index-<release>.tsv.gz      immutable
<root>/tl2026/packs-<release>.tsv.gz      immutable (each pack's SHA-256 and size)
<root>/tl2026/h/<hash>.pack               content-named, shared by the prefix's releases
```

**The font map a release ships** (the `pdftex-map` pack's
`fonts/map/pdftex/updmap/pdftex.map`) is updmap's output, not the `Map`
and `MixedMap` files of `updmap.cfg` joined. updmap (`updmap.pl`,
"Generating output for pdftex") puts first the 35 PostScript base fonts
from `ps2pk35.map` with their names and files translated for the `LW35`
mode (`URWkb` in TeX Live: `ptmr8r Times-Roman` becomes
`ptmr8r NimbusRomNo9L-Regu "TeXBase1Encoding ReEncodeFont" <8r.enc
<utmr8a.pfb`), as `pdftexDownloadBase14 true` asks, then the
`MixedMap` files, then the `Map` files, then `to_pdftex`. tl2026.7's map
lacks that first part: no `ptmr8r`, `phvr8r`, `pcrr8r`, … (1365 of TeX
Live's names, the base-35 families among them), so every document in
Times, Helvetica or Courier (IEEEtran, `\usepackage{times}`, psnfss)
fails on `Font ptmr8r not found` under `--shelf` and in the frontends'
engine; the engine reads `pdftex.map` alone, as pdfTeX does, and the
index has no `fonts/map/dvips/` file it could fall back on. Shelf's
`build.py` now runs TeX Live's own `updmap.pl` (with TeX Live's
`updmap.cfg`, Perl modules and `kpsewhich` of the same tlnet) over the
release's map files, so the map is the one an install of that tlnet has;
and Shelf's release check builds its documents with TeX Live's own
engines from the release's files alone, and fails if any font of TeX
Live's `pdftex.map` is missing from the release's. A quick look at a
deployed release:

```
gzip -dc h/<pdftex-map pack>.pack | grep -c '^ptmr8r '    # 1, not 0
```

- **The hostname carries the public protocol**, counted from 1 at the
  alpha: `v1.shelf.phitex.org` serves index schema 1. The schemas before it (1 to 4 of development)
  were never released.
- **Schema 3** is project `shelf-phitex` at `shelf.phitex.org` (tl2026.5).
  Nothing released reads it. It stays up for development builds of the
  extensions until they move, and PhiTeX drops schema 3 from `SCHEMAS`.
- **Schema 1** (the development schema 4, renumbered) is project `shelf-phitex-v1`. The development projects `shelf-phitex` and `shelf-phitex-v4` were never released.
  It gets the custom domain **`v1.shelf.phitex.org`**, which is the URL
  PhiTeX builds in.
- **A schema an older client can't read** gets the next project and
  domain (`v2.shelf.phitex.org`). The old project stays.
- **`schema`** is in each release's json. PhiTeX lists the schemas it reads
  (`phitex_shelf::SCHEMAS`, now 3 and 4). A release whose schema a client
  can't read is skipped by that client, which says so and names the version
  that reads it.
- **Limits:**
  - Pages' free plan serves 20,000 files a deployment, 25 MiB each.
  - By the README's measurements, that holds tl2026 and tl2027, roughly to
    spring 2028.
  - After that the packs move to R2 at the same URLs (10 GB free, no
    egress fees). Indexes and json stay on Pages.
  - Volumes (packs appended to big files, read by Range) don't work:
    Pages ignores Range (measured 2026-10-28).
- **Retiring a release** is a deliberate act, after its last PhiTeX is past
  support (§6). Until then, packs are never deleted while a kept index
  names them.

**news.json and install.sh** are static files of the site (PhiTeX.org repo,
`public/`, deployed with it to the Pages project `phitex-org`):
`https://phitex.org/news.json` and `https://phitex.org/install.sh`.

**Binaries** are GitHub release assets (free, no bandwidth limit, no CORS
needed for curl): `<target>.tar.gz`, `SHA256SUMS` and `SHA256SUMS.minisig`
for each version tag.

## 3. Which release a PhiTeX reads

- **By default it reads the release it was built with** (`shelf::RELEASE`).
  The same PhiTeX then gives the same bytes on every machine, which is what
  exact builds promise.
- **`shelf-release = "latest"`** in `phitex.toml` follows the prefix's
  `release.json`, among the schemas it reads. A name such as `"tl2026.7"` pins that
  release. `PHITEX_SHELF_RELEASE` overrides both.
- **The URLs are built in** (`shelf::HOSTS`): the domain
  (`v1.shelf.phitex.org`), then the same Pages project by Cloudflare's
  name (`shelf-phitex-v1.pages.dev`). Some networks (KAUST's, other
  universities') reset the TLS connection on phitex.org's names; a host
  that cannot be reached is passed over for the next for the rest of the
  process, and one that answers with anything, an error included, is the
  answer. The editor extensions keep the same list. `shelf-url` in
  `phitex.toml` and `PHITEX_SHELF_URL` replace the list, for a mirror or
  an offline copy. Packs are checked by SHA-256 whatever the host.
- **Caches are keyed by release and pack hash**, so two versions share one
  pack cache safely.

## 4. Updates and news

**Install:** `curl -fsSL https://phitex.org/install.sh | sh`. The script:
1. picks the target (x86_64/aarch64, linux/macos);
2. downloads the tarball, `SHA256SUMS` and `SHA256SUMS.minisig`;
3. verifies the signature against the public key in the script;
4. installs to `~/.local/bin/phitex` and writes `~/.local/share/phitex/install.json`
   (`{"method": "script", "version": ...}`).

`cargo install` and distribution packages are the other methods.

**`phitex upgrade`:**
- Does the same download and checks, with the public key built into the
  binary. It replaces the binary atomically (write beside it, then rename).
- Follows the user's `update-channel`: `stable` (the default: GitHub's
  latest release) or `nightly` (the pre-release tagged `nightly`, checked
  against the nightly key built in, `ci/nightly.pub`; a later stable
  release is taken instead). An install by `PHITEX_CHANNEL=nightly`
  install.sh records `"channel": "nightly"` in `install.json`, followed
  until the user sets `update-channel`.
- It runs only for the `script` method. Otherwise it prints the command for
  the method that installed it (`cargo install …`, the package manager).

**`news.json`** is written from `CHANGELOG.md` in the repo at each release (an entry is a `## <version> (<date>)` heading; its first paragraph is the summary). The binary also carries its own version's entry: the first run of a version in a terminal says the summary once (`$XDG_STATE_HOME/phitex/news` keeps the last version shown), and `phitex news` prints the whole entry:

```json
{ "schema": 1,
  "latest": "1.61",
  "entries": [
    { "version": "1.61", "date": "2026-11-15", "kind": "release",
      "summary": "XeLaTeX edits in milliseconds", "url": "https://…/CHANGELOG.md#020" },
    { "affects": "<1.618", "kind": "advisory",
      "summary": "1.6 and 1.61 can write a wrong PDF after deleting a page; upgrade" } ] }
```

- `kind` is `release` or `advisory`. An unknown kind is ignored.
- `affects` is a semver range. An advisory is shown only to the versions it
  names, and it is shown even when the version is otherwise current.

**The update check:**
- **The user chooses** (the user, 2026-10-30): `update-check = true` or
  `false` in the *user's* `phitex.toml` (`~/.config/phitex/`), never a
  project's; `phitex config update-check on|off` writes it. Until it is
  set, every build (after its result) and every watch (as it starts) says
  one dim line: `update checks are not set: phitex config update-check on
  (off: never ask again)`. No request is made until it is on
  (`crates/cli/src/upgrade.rs`).
- **At most once a day**, in the background, never delaying a build: the
  fetch the run starts is what the next run reads. The result is kept in
  the cache directory's `news.json`.
- **On stderr after a build's result, or as a watch starts**, from what
  was kept: each release after the running one with its summary (what
  the update brings), the advisories for the running version, and the
  command:
  ```
  phitex 1.618 is out (you have 1.6):
    1.618   SyncTeX for XeLaTeX.
    1.61    Collaborators' cursors.
    ⚠ 1.6 can write a wrong PDF.
  update: phitex upgrade
  ```
  The fetch starts as the run starts, so a short build reads it on its
  next run.
- **Never** under `CI`, with no TTY, under `phitex lsp`, or with
  `PHITEX_NO_UPDATE_CHECK=1`. The editor extension shows the same news
  through `phitex/status` (§7).
- **The request** is one GET of `https://phitex.org/news.json` (then
  `https://phitex-org.pages.dev/news.json` where phitex.org is not
  reached) with
  `User-Agent: phitex/<version>`. No identifiers, no query string.

## 5. Configuration compatibility

`phitex.toml` is shared in git between people on different versions.

- **Unknown keys are a warning, not an error.** Today `Config` uses
  `deny_unknown_fields`, so a file written for a newer PhiTeX fails on an
  older one. The fix: `unknown key 'foo' (a newer PhiTeX's? or did you mean
  'bar'?)`, with the closest known key suggested, and the build goes on.
- **No `config-version` number.** Keys keep their meaning forever.
  - A rename keeps the old name as an alias, as `file` and `program` already
    are.
  - A removed key becomes a warning that says what replaced it.
- **`phitex = ">=1.61"`** is the one versioned key. A project states the
  oldest PhiTeX it needs, like Cargo's `rust-version`. An older PhiTeX stops
  with `this project needs phitex >=1.61 (you have 1.6): phitex upgrade`.
  This turns a confusing failure into an instruction.
- **Values grow, never change.** An enum gains variants. An older PhiTeX
  rejects a variant it doesn't know with an error naming the key and the
  values it knows.

## 6. Versions and what is kept

- **Versions converge to φ** (AGENTS.md): 1.6 is the alpha, then 1.61,
  1.618, …, a digit a release, fixes included. `crates/cli/Cargo.toml`
  carries it as `1.6.0`, `1.61.0`, …, so semver orders them (the minor
  grows); it is named and tagged without the `.0` (`phitex 1.61`,
  `v1.61`). `crates/cli/Cargo.toml` is `0.0.0` until the release.
- **`phitex --version`** prints the version, commit, engine id,
  `RECORDS_VERSION`, Shelf protocol and schemas, and editor protocol.
- **Caches** are already gated by engine id and `RECORDS_VERSION`. A cache
  directory from another engine id is never read. `phitex cache clean` also
  removes directories older engines left.
- **Support window:** the server keeps every release that a version
  published in the last 12 months pins. `shelf.json` keeps listing older
  ones until they are retired; retiring means removing the entry and
  advising (`news.json`) the versions that pinned it.

## 7. Editors

- **Each side sends its version in LSP `initialize`.** The extension sends
  its own in `initializationOptions.client`; `phitex lsp` answers
  `serverInfo.version` and `capabilities.experimental.phitex.protocol`
  (an integer).
- **The extension needs a minimum PhiTeX.** If `phitex lsp` is older, it
  says `PhiTeX 1.6 is older than this extension needs (1.61): run phitex
  upgrade`.
- **Shelf's `min_extension`** (already in release.json) is read the same
  way.
- **`phitex/status`** carries the update line and any advisory, when the
  user has turned the check on.

## 8. What the user provides

- **A host and domain** for the static root.
- **A minisign key pair.** It is generated and kept by the user; only the
  public key goes in the repo. The secret key never enters a session.
- **Where binaries are built:** the GitHub Actions release workflow, or mc,
  then uploaded.

## 9. Work, in order

1. **CLI:**
   - `shelf::HOSTS` (`https://v1.shelf.phitex.org/tl2026/`, then `https://shelf-phitex-v1.pages.dev/tl2026/`);
   - `shelf-url` and `shelf-release` keys;
   - skip unreadable schemas with a message;
   - warn on unknown config keys;
   - the `phitex` version key;
   - version 1.6 (Cargo 1.6.0) and the `--version` lines.
2. **Shelf:**
   - add the custom domain `v1.shelf.phitex.org` to the Pages project `shelf-phitex-v1` (made by the first deploy);
   - build tl2026.7 from the xetex-trees branch on mc, with TRACE and FONT_INDEX;
   - `release.py` writes `shelf.json`;
   - tl2026.8: `build.py` makes `pdftex.map` with TeX Live's `updmap.pl` (§2: tl2026.7's lacks the LW35 fonts, so Times documents fail), checked by the release check against TeX Live before it is deployed;
   - `deploy.sh` to `shelf-phitex-v1`.
3. **`CHANGELOG.md` → `news.json`** generator, the update check and `phitex
   upgrade`, and the release workflow (targets, `SHA256SUMS`, minisign).
4. **`install.sh`.**
5. **Editor:** the version exchange in `initialize`, and status lines.
