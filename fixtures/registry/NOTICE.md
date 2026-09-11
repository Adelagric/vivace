# Frozen Packagist snapshots

One archive per fixture, produced by `tools/snapshot-packagist.sh`: every
`p2/<vendor>/<name>[~dev].json` file Composer loaded while resolving the
fixture's manifest (its own cache after a `composer update --no-install` with
an empty cache), the reference `composer.lock.expected` Composer wrote from
live Packagist at that moment, and a `SNAPSHOT` file with the date, the
Composer version and the virtual package names (404 on Packagist, stubbed as
"no versions" because a missing file is fatal over `file://`).

`harness/update.sh` unpacks an archive, points both Composer and vivace at it
through the global config (`COMPOSER_HOME/config.json`, so `composer.json`
stays byte-identical and its content-hash counts), and compares the locks.
The metadata is public Packagist data (package manifests published by their
authors); it is test data, not part of vivace. `wordpress` has no snapshot:
wpackagist speaks the Composer v1 provider protocol, out of scope for the
resolver.
