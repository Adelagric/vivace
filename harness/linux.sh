#!/usr/bin/env bash
# Toute la chaîne (gates, fixtures, tests oracle, parité, boot) dans un
# conteneur Linux, avec le code monté et les caches persistés dans des volumes.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
docker build -q -t vivace-linux "$ROOT/harness/linux" >/dev/null
docker run --rm -t \
  -v "$ROOT:/src:ro" \
  -v vivace-linux-cargo:/root/.cargo/registry \
  -v vivace-linux-target:/work/target \
  -v vivace-linux-fixtures:/work/fixtures/work \
  -v vivace-linux-cache:/root/.cache \
  -v vivace-linux-composer:/root/.composer \
  vivace-linux bash -c '
    set -euo pipefail
    # copie du code (montage en lecture seule ; target/ et fixtures/work sont des volumes)
    (cd /src && tar --exclude=./target --exclude=./fixtures/work --exclude=./.git -cf - .) | (cd /work && tar -xf -)
    cd /work
    echo "== gates"
    cargo fmt --check
    cargo clippy --all-targets -q -- -D warnings
    cargo build --release -q
    echo "== fixtures"; fixtures/make.sh
    echo "== tests"; cargo test -q 2>&1 | grep -E "result:|FAILED|panicked"
    echo "== parity"; harness/diff-vendor.sh
    echo "== parity + autoload (froid puis chaud)"; harness/diff-vendor.sh --with-autoloader; harness/diff-vendor.sh --with-autoloader
    echo "== boot"; harness/boot.sh
    echo "== clone strategy check"; ls -li /tmp/vivace-harness/viv-laravel/vendor/monolog/monolog/composer.json /root/.cache/vivace/store/monolog/monolog/*/composer.json | head -3
  '
