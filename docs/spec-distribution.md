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

## 2. The server (static files)

One root, `https://shelf.phitex.org/`: a Cloudflare R2 bucket on the phitex.org zone, cached by Cloudflare in front (free: 10 GB stored, 10M reads and 1M writes a month, no egress charge; cache hits are not R2 reads). R2 sets CORS, which the browser viewer's Shelf needs. Everything is static, so
any file host or CDN works.

```
/v1/shelf.json                      the only mutable file (short cache)
/v1/news.json                       mutable, short cache (§4)
/v1/packs/<sha256>.pack             content-named, shared by all releases
/v1/releases/<release>/release.json immutable (schema, search, deps, notice)
/v1/releases/<release>/index-<release>.tsv.gz
/v1/releases/<release>/packs-<release>.tsv.gz
```

Binaries are GitHub release assets (free, no bandwidth limit, no CORS
needed for curl): `<target>.tar.gz`, `SHA256SUMS`, `SHA256SUMS.minisig` per
version tag. `https://phitex.org/install.sh` and `phitex upgrade` fetch them
from there. Storage budget: R2's 10 GB holds the current release (1.8 GB) and
the shared packs of the releases in the support window (§6); a release
re-uploads only its changed packs.

`shelf.json`:

```json
{ "protocol": 1,
  "releases": [ { "name": "tl2026.7", "schema": 4, "date": "2026-10-30" } ],
  "latest": "tl2026.7" }
```

- **`/v1/`** is the protocol: the URL layout and the meaning of these files.
  A change an older PhiTeX could misread goes to `/v2/`, with `/v1/` kept.
- **`schema`** is a release index's format. PhiTeX lists the schemas it reads
  (`phitex_shelf::SCHEMAS`, now 3 and 4). A release whose schema a client
  can't read is skipped by that client, which says so and names the version
  that reads it.
- **Packs are named by their SHA-256.** A new TeX Live release re-uploads
  only changed packs (tl2026.7 kept 8365 and added 1396). Old releases cost
  only their indexes.
- Packs are never deleted while a release that lists them is served.
  Retiring a release is a deliberate act, after its last PhiTeX is past
  support (§6).

## 3. Which release a PhiTeX reads

- **By default it reads the release it was built with** (`shelf::RELEASE`).
  The same PhiTeX then gives the same bytes on every machine, which is what
  exact builds promise.
- **`shelf-release = "latest"`** in `phitex.toml` follows `shelf.json`'s
  `latest`, among the schemas it reads. A name such as `"tl2026.7"` pins that
  release. `PHITEX_SHELF_RELEASE` overrides both.
- **The URL is built in** (`shelf::DEFAULT_URL`). `shelf-url` in
  `phitex.toml` and `PHITEX_SHELF_URL` override it, for a mirror or an
  offline copy. Packs are checked by SHA-256 whatever the host.
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
- It runs only for the `script` method. Otherwise it prints the command for
  the method that installed it (`cargo install …`, the package manager).
- `phitex upgrade --check` reports without installing.

**`news.json`** is written from `NEWS.md` in the repo at each release:

```json
{ "schema": 1,
  "latest": "0.2.0",
  "entries": [
    { "version": "0.2.0", "date": "2026-11-15", "kind": "release",
      "summary": "XeLaTeX edits in milliseconds", "url": "https://…/NEWS.md#020" },
    { "affects": "<0.1.3", "kind": "advisory",
      "summary": "0.1.0-0.1.2 can write a wrong PDF after deleting a page; upgrade" } ] }
```

- `kind` is `release` or `advisory`. An unknown kind is ignored.
- `affects` is a semver range. An advisory is shown only to the versions it
  names, and it is shown even when the version is otherwise current.

**The update check:**
- **Off by default.** It is turned on with `update-check = true` in the
  *user's* `phitex.toml` (never a project's), or `phitex config
  update-check on`. The first interactive `phitex watch` asks once, by
  default no, and remembers the answer.
- **At most once a day**, in the background, never delaying a build. The
  result is cached in `~/.cache/phitex/news.json`.
- **One line on stderr after a build:** `phitex 0.2.0 is out (you have
  0.1.4): phitex upgrade`. An advisory for the running version is printed
  even if the release line was already shown.
- **Never** under `CI`, with no TTY, under `phitex lsp`, or with
  `PHITEX_NO_UPDATE_CHECK=1`. The editor extension shows the same news
  through `phitex/status` (§7).
- **The request** is one GET of `/v1/news.json` with
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
- **`phitex = ">=0.2"`** is the one versioned key. A project states the
  oldest PhiTeX it needs, like Cargo's `rust-version`. An older PhiTeX stops
  with `this project needs phitex >=0.2 (you have 0.1.4): phitex upgrade`.
  This turns a confusing failure into an instruction.
- **Values grow, never change.** An enum gains variants. An older PhiTeX
  rejects a variant it doesn't know with an error naming the key and the
  values it knows.

## 6. Versions and what is kept

- **The CLI follows semver from 0.1.0.** Alpha is `0.1.x`.
  `crates/cli/Cargo.toml` is `0.0.0` today.
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
  says `PhiTeX 0.1.4 is older than this extension needs (0.2): run phitex
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
   - `shelf::DEFAULT_URL` and the `/v1/` layout;
   - `shelf-url` and `shelf-release` keys;
   - skip unreadable schemas with a message;
   - warn on unknown config keys;
   - the `phitex` version key;
   - version 0.1.0 and the `--version` lines.
2. **Shelf `build.py`:**
   - write the `/v1/` layout (`packs/<sha256>.pack` shared, `releases/<r>/`);
   - update `shelf.json` (merge, never drop a release);
   - TRACE and FONT_INDEX on.
3. **`NEWS.md` → `news.json`** generator, the update check and `phitex
   upgrade`, and the release workflow (targets, `SHA256SUMS`, minisign).
4. **`install.sh`.**
5. **Editor:** the version exchange in `initialize`, and status lines.
