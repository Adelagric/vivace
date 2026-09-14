#!/usr/bin/env bash
# Toute la chaîne (gates, fixtures, tests oracle, parité, boot) dans un
# conteneur Linux, avec le code monté et les caches persistés dans des volumes.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
docker build -q -t vivacity-linux "$ROOT/harness/linux" >/dev/null
docker run --rm -t \
  -v "$ROOT:/src:ro" \
  -v vivacity-linux-cargo:/root/.cargo/registry \
  -v vivacity-linux-target:/work/target \
  -v vivacity-linux-fixtures:/work/fixtures/work \
  -v vivacity-linux-cache:/root/.cache \
  -v vivacity-linux-composer:/root/.composer \
  vivacity-linux bash -c '
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
    echo "== clone strategy check"; ls -li /tmp/vivacity-harness/viv-laravel/vendor/monolog/monolog/composer.json /root/.cache/vivacity/store/monolog/monolog/*/composer.json | head -3
  '
