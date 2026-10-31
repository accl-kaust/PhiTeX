#!/bin/bash
# Apptainer for a cluster whose nodes have none (mc, Ubuntu 22.04): the
# Apptainer project's own Ubuntu build for jammy, from its Launchpad PPA
# (ppa:apptainer/ppa), unpacked without root into
# ~/shared-cache/apptainer-VER and linked as ~/shared-cache/apptainer.
# (The GitHub release's .deb is built for Ubuntu 24.04: it needs
# libsubid.so.4 and FUSE_3.12, which 22.04 has not.)
# Checked before unpacking: the PPA's InRelease against the PPA's
# signing key (fingerprint pinned, the key from keyserver.ubuntu.com),
# Packages.gz against InRelease's SHA-256, the .deb against Packages'
# SHA-256, which is pinned here too.
# A non-setuid Apptainer is relocatable once its etc and var sit beside
# bin and libexec under one prefix (usr/); it runs unprivileged (user
# namespaces, SIFs mounted by its own squashfuse_ll). job.sbatch,
# corpus-sweep.sbatch, ci.sbatch and build-image.sbatch put its usr/bin
# on PATH when the node has no apptainer. Once per cluster, in a job
# (never on the login node):
#   sbatch -p mc -C cpu_amd_epyc_9754 -c 2 --mem=4G -t 00:20:00 apptainer-user.sbatch
#SBATCH -J apptainer -o apptainer-%j.out
set -euo pipefail
ver=1.5.5-1~jammy
sha=94d913c3a4baf599798b002f0c4283047c638f764b879529f17b63a4dfa9dbd5
fpr=F6B0F5193D4F3301EF491FF0AFE36534FC6218AE
ppa=https://ppa.launchpadcontent.net/apptainer/ppa/ubuntu
d=$HOME/shared-cache/apptainer-${ver%%-*}
w=/tmp/$USER-$SLURM_JOB_ID
mkdir -p "$w"
trap 'rm -rf "$w"' EXIT
cd "$w"
fail() { echo "FAILED: $*"; exit 1; }
# the index, signed
curl -fsS --retry 3 -o key.asc "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x$fpr"
mkdir -m 700 "$w/gnupg"
gpg -q --homedir "$w/gnupg" --batch --import key.asc
gpg --homedir "$w/gnupg" --batch --with-colons --fingerprint | grep -q "^fpr:::::::::$fpr:" || fail "key $fpr not imported"
gpg --homedir "$w/gnupg" --batch --export "$fpr" > key.gpg
curl -fsS --retry 3 -o InRelease "$ppa/dists/jammy/InRelease"
gpgv --keyring "$w/key.gpg" InRelease 2>&1 | tail -2
gpgv --keyring "$w/key.gpg" InRelease > /dev/null 2>&1 || fail "InRelease not signed by $fpr"
curl -fsS --retry 3 -o Packages.gz "$ppa/dists/jammy/main/binary-amd64/Packages.gz"
grep -q " $(sha256sum Packages.gz | cut -d' ' -f1) .* main/binary-amd64/Packages.gz$" InRelease || fail "Packages.gz not as InRelease lists it"
idx=$(gunzip -c Packages.gz | awk -v v="$ver" '/^Package: /{p=$2} /^Version: /{q=$2} /^SHA256: /{if (p=="apptainer" && q==v) print $2}')
[ "$idx" = "$sha" ] || fail "the index lists apptainer $ver with sha256 '$idx', pinned $sha"
# the package
deb=apptainer_${ver}_amd64.deb
curl -fsS --retry 3 -o "$deb" "$ppa/pool/main/a/apptainer/${deb/\~/%7E}"
got=$(sha256sum "$deb" | cut -d' ' -f1)
[ "$got" = "$sha" ] || fail "$deb sha256 $got, the signed index lists $sha"
echo "$deb sha256 $got (as the PPA's signed index lists it)"
rm -rf "$d.part"
mkdir -p "$d.part"
dpkg-deb -x "$deb" "$d.part"
for x in etc var; do [ ! -d "$d.part/$x" ] || mv "$d.part/$x" "$d.part/usr/"; done
rm -rf "$d"
mv -T "$d.part" "$d"
ln -sfn "$(basename "$d")" "$HOME/shared-cache/apptainer"
export PATH=$d/usr/bin:$PATH
apptainer version
for f in "$d"/usr/bin/apptainer "$d"/usr/libexec/apptainer/bin/*; do
  m=$(ldd "$f" 2>&1 | grep -E 'not found|version .* not found' | tr '\n' ' ')
  [ -z "$m" ] || echo "$(basename "$f"): $m"
done
