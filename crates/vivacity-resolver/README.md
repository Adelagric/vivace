# vivacity-resolver

Composer 2.10.3's dependency resolution ported function by function for [vivacity](https://github.com/Adelagric/vivacity): composer/semver, Packagist v2 metadata with Composer's cache layout, pool builder and optimizer, rule set, the CDCL solver, the default policy, the lock transaction and lock writer, plus JsonManipulator and VersionSelector for require/remove. Checked against Composer on frozen Packagist snapshots (same candidate pool, same solver decisions, same lock).

License: MIT or Apache-2.0. The code ported from Composer and its libraries is © Nils Adermann, Jordi Boggiano and the Composer project (MIT); see NOTICE.md in the repository.
